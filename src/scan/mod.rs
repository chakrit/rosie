//! Scanning: finding what the rules match and building the plan from it
//! (`docs/spec/cli.md#modes`, `docs/spec/performance.md#scan`).
//!
//! - `tree <dir>` walks the folder in parallel, applying the folder rules and the fixed
//!   rule paths that lie under it. The walk stops descending at a match, never enters or
//!   matches a symlink, and prunes the walk skips (`docs/spec/safety.md#walk-skips`).
//! - `caches` takes the fixed rule paths and the tool rules.
//!
//! Every target is sized as soon as it is found, while the walk goes on, and is marked
//! `outside-roots` when outside the roots and `sudo` when not owned by the user. Every
//! target passes the context's delete checks: one a running process runs a program
//! from is refused and listed apart from the plan.

mod entry;
mod fixed;
mod log;
mod progress;
mod refusal;
mod sizing;
mod targets;
mod walk;

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::Walk;
use crate::fs::{self, Backend, Cleanup, FileKind, FsContext, FsOp, Paths, UserInput};
use crate::plan::{self, AggressiveItems, AgreedTiers, Plan, PlanBuilder, ToolCmds, WalkSkips};
use crate::rules::{RuleId, RuleSet, Shape, Tier, Twin};

pub use entry::{Entry, Folder, InFolder, Mounts, NotAFolder, NotAnEntry, Sealed};
pub use log::{Log, Problem, Skipped};
pub use progress::{NoProgress, Progress, StderrProgress};
pub use refusal::Refused;
pub(crate) use refusal::{RefusedItems, Verdict, screen};
pub use sizing::{Measure, Sizer};

use fixed::{FixedClaim, FixedTargets};
use targets::Targets;
use walk::{TreeWalk, Walked};

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error("{path} is not a folder", path = path.display())]
    NotAFolder { path: PathBuf },

    /// The path [`Scanner::path`] was given is refused, such as one through a symlink
    /// or spelled differently from the disk, or cannot be looked at.
    #[error(transparent)]
    PathRefused(fs::Error),

    #[error("{path} does not exist", path = path.display())]
    Missing { path: PathBuf },
}

/// What a scan is told beyond the rules: the injected home for `~`, the walk flags
/// (config `[walk]` with the CLI flags applied), and whether `--aggressive` was given.
#[derive(Debug, Clone)]
pub struct Settings {
    pub paths: Paths,
    pub walk: Walk,
    pub aggressive: AggressiveItems,
}

/// What one scan found, in any mode: tree and caches here, app and orphans in
/// [`crate::apps`].
#[derive(Debug)]
pub struct Scan {
    pub plan: Plan,
    /// The tier the rules of each delete entry agree on; a conflict entry has none.
    pub tiers: AgreedTiers,
    /// Items a running process executes from, kept out of the plan, sorted by path.
    pub refused: Vec<Refused>,
    /// Paths the walk or the sizing did not enter, sorted by path.
    pub skipped: Vec<Skipped>,
    /// Errors that cost the scan an item or a rule check, such as a fixed rule path
    /// through a symlink or a failing Lua rule. The rest of the scan went on.
    pub problems: Vec<Problem>,
}

/// What [`Scanner::path`] found: where the typed path is on disk, and the scan.
#[derive(Debug)]
pub struct PathScan {
    pub path: PathBuf,
    pub scan: Scan,
}

impl Scan {
    /// Walk-skip counts per reason, for the stats.
    pub fn skip_counts(&self) -> WalkSkips {
        self.skipped.iter().map(|skipped| skipped.reason).collect()
    }
}

/// One rule accepting a target, under the tier its field came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuleMatch {
    pub rule: RuleId,
    pub tier: Tier,
}

pub struct Scanner<B: Backend> {
    ctx: FsContext<B>,
    rules: RuleSet,
    settings: Settings,
}

impl<B: Backend + Sync> Scanner<B> {
    pub fn new(ctx: FsContext<B>, rules: RuleSet, settings: Settings) -> Self {
        Scanner {
            ctx,
            rules,
            settings,
        }
    }

    /// Scans `dir`, a path the user typed: refused when it passes through a symlink or
    /// is spelled differently from the disk. The folder itself is never a target; the
    /// walk looks at everything below it. A folder that is a dataless placeholder is
    /// not opened unless the walk may enter placeholders.
    pub fn tree<P: Progress>(&self, dir: &Path, progress: &P) -> Result<Scan, Error> {
        let root = UserInput::path(dir, self.settings.walk.enter_links).perform(&self.ctx)?;
        let root_meta = Cleanup::lstat(&root).perform(&self.ctx)?;
        if root_meta.kind != FileKind::Dir {
            return Err(Error::NotAFolder { path: root });
        }
        self.ctx.snapshot_running()?;
        let log = Log::default();
        let links = self.settings.walk.enter_links;
        let fixed = FixedTargets::of(&self.rules, &self.settings.paths, &self.ctx, links);
        let fixed = fixed::by_path(fixed.under(&root).report_refused(&log));

        let targets = Targets::new(&self.ctx, &log, progress);
        let walk = TreeWalk {
            ctx: &self.ctx,
            rules: &self.rules,
            fixed: &fixed,
            flags: self.settings.walk,
            targets: &targets,
            log: &log,
            walked: Walked::starting_at(&root, links),
        };
        rayon::scope(|scope| walk.start(scope, root, root_meta));

        let found = targets.finish();
        let builder = PlanBuilder::new(self.settings.aggressive);
        Ok(self.assemble(builder, found, log))
    }

    /// Scans what a plan would hold at, above, and below `path`, a path the user typed,
    /// refused as [`Scanner::tree`] refuses its folder. The walk starts where a fixed
    /// path's volume check does, at home for a path under it and at `/` otherwise, and
    /// goes down toward `path`, judging each folder on the way as a `tree` walk would; at
    /// `path` it takes the entry as any other, so a folder it would enter is walked whole
    /// (`docs/spec/cli.md#check`). Only the fixed rule paths on that way, or at or below
    /// `path`, are looked at.
    pub fn path<P: Progress>(&self, path: &Path, progress: &P) -> Result<PathScan, Error> {
        let links = self.settings.walk.enter_links;
        let real = UserInput::path(path, links)
            .perform(&self.ctx)
            .map_err(Error::PathRefused)?;
        match Cleanup::lstat(&real).perform(&self.ctx) {
            Ok(_) => {}
            Err(error) if error.has_vanished() => return Err(Error::Missing { path: real }),
            Err(error) => return Err(Error::PathRefused(error)),
        }
        self.ctx.snapshot_running()?;
        let anchor = entry::anchor(&real, &self.settings.paths).to_path_buf();
        let anchor_meta = Cleanup::lstat(&anchor).perform(&self.ctx)?;
        let log = Log::default();
        let fixed = FixedTargets::of(&self.rules, &self.settings.paths, &self.ctx, links);
        let fixed = fixed.under(&anchor).along(&real);
        let fixed = fixed::by_path(fixed.report_refused(&log));

        let targets = Targets::new(&self.ctx, &log, progress);
        let walk = TreeWalk {
            ctx: &self.ctx,
            rules: &self.rules,
            fixed: &fixed,
            flags: self.settings.walk,
            targets: &targets,
            log: &log,
            walked: Walked::starting_at(&anchor, links),
        };
        rayon::scope(|scope| match real == anchor {
            true => walk.start(scope, anchor, anchor_meta),
            false => walk.visit_toward(scope, anchor, anchor_meta, &real),
        });

        let found = targets.finish();
        let builder = PlanBuilder::new(self.settings.aggressive);
        let scan = self.assemble(builder, found, log);
        Ok(PathScan { path: real, scan })
    }

    /// Scans the fixed rule paths, with `~` taken as the injected home, and lists the
    /// tool rules' commands. An absent path is not planned.
    pub fn caches<P: Progress>(&self, progress: &P) -> Result<Scan, Error> {
        self.ctx.snapshot_running()?;
        let log = Log::default();
        let links = self.settings.walk.enter_links;
        let fixed = FixedTargets::of(&self.rules, &self.settings.paths, &self.ctx, links);
        let fixed = fixed.outermost().report_refused(&log);

        let targets = Targets::new(&self.ctx, &log, progress);
        let claim = FixedClaim {
            ctx: &self.ctx,
            paths: &self.settings.paths,
            mounts: self.settings.walk.into(),
            targets: &targets,
            log: &log,
        };
        rayon::scope(|scope| {
            for (path, rules) in fixed.into_iter() {
                let claim = &claim;
                scope.spawn(move |scope| claim.claim(scope, path, rules));
            }
        });

        let found = targets.finish();
        let mut builder = PlanBuilder::new(self.settings.aggressive);
        self.add_tools(&mut builder);
        Ok(self.assemble(builder, found, log))
    }

    // plan

    fn add_tools(&self, builder: &mut PlanBuilder) {
        for rule in self.rules.iter() {
            let Shape::Tool(commands) = rule.shape() else {
                continue;
            };
            let cmds = match commands {
                Twin::Normal(cmd) => ToolCmds::Normal(cmd),
                Twin::Aggressive(cmd) => ToolCmds::Aggressive(cmd),
                Twin::Both { normal, aggressive } => ToolCmds::Twins {
                    cmd: normal,
                    cmd_aggressive: aggressive,
                },
            };

            builder.add_tool(&rule.id().to_string(), cmds);
        }
    }

    fn assemble(&self, mut builder: PlanBuilder, found: targets::Found, log: Log) -> Scan {
        for target in found.targets {
            for path_match in target.path_matches() {
                if let Err(error) = builder.add_path(path_match) {
                    log.problem(Problem::Plan(error));
                }
            }
        }

        let (mut skipped, problems) = log.into_parts();
        skipped.sort_by(|a, b| a.path.cmp(&b.path));
        let built = builder.build();
        Scan {
            plan: built.plan,
            tiers: built.tiers,
            refused: found.refused,
            skipped,
            problems,
        }
    }
}

/// The plan's name for a rule field's tier.
fn plan_twin(tier: Tier) -> plan::Twin {
    match tier {
        Tier::Normal => plan::Twin::Normal,
        Tier::Aggressive => plan::Twin::Aggressive,
    }
}
