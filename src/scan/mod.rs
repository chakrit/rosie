//! Scanning: finding what the rules match and building the plan from it
//! (`docs/spec/cli.md#modes`, `docs/spec/performance.md#scan`).
//!
//! - `tree <dir>` walks the folder in parallel, applying the folder rules and the fixed
//!   rule paths that lie under it. The walk stops descending at a match, never enters or
//!   matches a symlink, and prunes the walk skips (`docs/spec/safety.md#walk-skips`).
//! - `caches` takes the fixed rule paths and the tool rules.
//!
//! Every target is sized as soon as it is found, while the walk goes on, and is marked
//! `skip` when outside the roots and `sudo` when not owned by the user. Every
//! target passes the context's delete checks: one a running process runs a program
//! from is refused and listed apart from the plan.

mod entry;
mod fixed;
mod log;
mod progress;
mod screening;
mod sizing;
mod skip;
mod targets;
mod verdict;
mod walk;

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::Walk;
use crate::fs::{
    self, Backend, Cleanup, FileKind, FsContext, FsOp, Metadata, PathPattern, UserInput,
};
use crate::plan::{AggressiveItems, EntryTiers, Plan, PlanBuilder};
use crate::rules::{RuleId, RuleSet, Shape};
use crate::tier::Tier;
use crate::wildcard::Pattern;

pub use entry::{Entry, Folder, FolderError, InFolder, Mounts, NotAnEntry, Sealed};
pub use log::{Log, Problem, Skipped};
pub use progress::{NoProgress, Progress};
pub use screening::Refused;
pub(crate) use screening::Screening;
pub use sizing::{Measure, Sizer};
pub use skip::{Needs, WalkSkip, WalkSkips};
pub(crate) use verdict::{PathVerdict, Place, Planned};

use fixed::{FixedClaim, FixedPaths, FixedTargets};
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

    /// A `tree <dir>` pattern that names no folder (`docs/spec/cli.md#patterns`).
    #[error("no folder matches {pattern}", pattern = pattern.display())]
    NoFolderMatches { pattern: PathBuf },
}

/// What a scan is told beyond the rules and its context: the walk flags (config `[walk]`
/// with the CLI flags applied), and whether `--aggressive` was given.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub walk: Walk,
    pub aggressive: AggressiveItems,
}

/// What one scan found, in any mode: tree and caches here, app and orphans in
/// [`crate::apps`].
#[derive(Debug)]
pub struct Scan {
    pub plan: Plan,
    /// Each delete entry's tier.
    pub tiers: EntryTiers,
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
    /// Builds the plan, and keeps what the scan met beside it with the skips sorted by
    /// path.
    pub(crate) fn new(
        builder: PlanBuilder,
        refused: Vec<Refused>,
        mut skipped: Vec<Skipped>,
        problems: Vec<Problem>,
    ) -> Scan {
        skipped.sort_by(|a, b| a.path.cmp(&b.path));
        let built = builder.build();
        Scan {
            plan: built.plan,
            tiers: built.tiers,
            refused,
            skipped,
            problems,
        }
    }

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

    /// The context every scan of this scanner works through, with its one
    /// running-program snapshot.
    pub fn ctx(&self) -> &FsContext<B> {
        &self.ctx
    }

    /// Scans `dir`, a path the user typed, or every folder it matches when one of its
    /// components holds a wildcard, into one plan (`docs/spec/cli.md#patterns`). Each folder is
    /// refused when it passes through a symlink or is spelled differently from the disk.
    /// A folder itself is never a target; the walk looks at everything below it. A
    /// folder that is a dataless placeholder is not opened unless the walk may enter
    /// placeholders.
    pub fn tree<P: Progress>(
        &self,
        dir: &PathPattern<Pattern>,
        progress: &P,
    ) -> Result<Scan, Error> {
        let folders = match dir.has_wildcard() {
            true => self.matching_folders(dir)?,
            false => vec![dir.path().to_path_buf()],
        };
        let ways = folders
            .iter()
            .map(|folder| self.tree_way(folder))
            .collect::<Result<Vec<_>, _>>()?;

        self.beside_snapshot(|| self.walk(&ways, progress))
    }

    /// A `tree` walk of `dir`, checked as typed.
    fn tree_way(&self, dir: &Path) -> Result<Way, Error> {
        let root = UserInput::path(dir, self.settings.walk.enter_links).perform(&self.ctx)?;
        let root_meta = match Cleanup::lstat(&root).perform(&self.ctx) {
            Ok(meta) => meta,
            Err(error) if error.has_vanished() => return Err(Error::Missing { path: root }),
            Err(error) => return Err(error.into()),
        };
        if root_meta.kind != FileKind::Dir {
            return Err(Error::NotAFolder { path: root });
        }

        Ok(Way::Whole { root, root_meta })
    }

    /// The folders `pattern` matches: the literal folder above its
    /// first wildcard is checked as typed, and the components below match ignoring case.
    fn matching_folders(&self, pattern: &PathPattern<Pattern>) -> Result<Vec<PathBuf>, Error> {
        let links = self.settings.walk.enter_links;

        let folders: Vec<PathBuf> = pattern
            .expand_typed(&self.ctx, links)?
            .into_iter()
            .filter(|&(_, kind)| kind == FileKind::Dir)
            .map(|(folder, _)| folder)
            .collect();
        match folders.is_empty() {
            true => Err(Error::NoFolderMatches {
                pattern: pattern.path().to_path_buf(),
            }),
            false => Ok(folders),
        }
    }

    /// Scans what a plan would hold at, above, and below `path`, a path the user typed,
    /// refused as [`Scanner::tree`] refuses its folder. The walk starts where a fixed
    /// path's volume check does, at home for a path under it and at `/` otherwise, and
    /// goes down toward `path`, judging each folder on the way as a `tree` walk would; at
    /// `path` it takes the entry as any other, so a folder it would enter is walked whole
    /// (`docs/spec/cli.md#check`), unless `path` is where the walk starts, which is
    /// walked as a `tree` walk is and is never a target itself. Only the fixed rule paths
    /// on that way, or at or below `path`, are looked at.
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
        let anchor = entry::anchor(&real, self.ctx.paths()).to_path_buf();
        let anchor_meta = Cleanup::lstat(&anchor).perform(&self.ctx)?;

        let way = match anchor == real {
            true => Way::Whole {
                root: anchor,
                root_meta: anchor_meta,
            },
            false => Way::Toward {
                start: anchor,
                start_meta: anchor_meta,
                target: real.clone(),
            },
        };
        let scan = self.beside_snapshot(|| self.walk(&[way], progress))?;
        Ok(PathScan { path: real, scan })
    }

    /// Scans the fixed rule paths, with `~` taken as the injected home, and lists the
    /// tool rules' commands. An absent path is not planned.
    pub fn caches<P: Progress>(&self, progress: &P) -> Result<Scan, Error> {
        self.beside_snapshot(|| self.claim_fixed(progress))
    }

    /// Claims the outermost fixed rule paths, and lists the tool rules' commands.
    fn claim_fixed<P: Progress>(&self, progress: &P) -> Scan {
        let log = Log::default();
        let links = self.settings.walk.enter_links;
        let fixed = FixedTargets::of(&self.rules, &self.ctx, links);
        let fixed = fixed.outermost().report_refused(&log);

        let targets = Targets::new(&self.ctx, &log, progress);
        let claim = FixedClaim {
            ctx: &self.ctx,
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
        self.assemble(builder, found, log)
    }

    /// Runs `scan` beside the running-program snapshot, which the scan's delete checks
    /// wait for; a scan that cannot read the snapshot fails.
    fn beside_snapshot<T: Send>(&self, scan: impl FnOnce() -> T + Send) -> Result<T, Error> {
        let (snapshot, scanned) = rayon::join(|| self.ctx.snapshot_running(), scan);
        snapshot?;
        Ok(scanned)
    }

    /// Walks every way into one scan. Only the fixed rule paths some way meets are
    /// looked at.
    fn walk<P: Progress>(&self, ways: &[Way], progress: &P) -> Scan {
        let log = Log::default();
        let links = self.settings.walk.enter_links;
        let fixed = FixedTargets::of(&self.rules, &self.ctx, links);
        let fixed = FixedPaths::of(fixed.met_on(ways).report_refused(&log));

        let targets = Targets::new(&self.ctx, &log, progress);
        let walk = TreeWalk {
            ctx: &self.ctx,
            rules: &self.rules,
            fixed: &fixed,
            flags: self.settings.walk,
            targets: &targets,
            log: &log,
            walked: Walked::starting_at(ways.iter().map(Way::start), links),
        };

        rayon::scope(|scope| {
            for way in ways {
                match way {
                    Way::Whole { root, root_meta } => walk.start(scope, root.clone(), *root_meta),
                    Way::Toward {
                        start,
                        start_meta,
                        target,
                    } => walk.visit_toward(scope, start.clone(), *start_meta, target),
                }
            }
        });

        let found = targets.finish();
        let builder = PlanBuilder::new(self.settings.aggressive);
        self.assemble(builder, found, log)
    }

    // plan

    fn add_tools(&self, builder: &mut PlanBuilder) {
        for rule in self.rules.iter() {
            let Shape::Tool(commands) = rule.shape() else {
                continue;
            };
            builder.add_tool(&rule.id().to_string(), commands);
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

        let (skipped, problems) = log.into_parts();
        Scan::new(builder, found.refused, skipped, problems)
    }
}

/// One way a walk goes. Each start's own `lstat` rides along.
pub(super) enum Way {
    /// A `tree` walk of `root`: everything below it, never `root` itself.
    Whole { root: PathBuf, root_meta: Metadata },
    /// From `start` down toward `target`, below it, judging each folder on the way as a
    /// `tree` walk would, and taking `target` as any other entry.
    Toward {
        start: PathBuf,
        start_meta: Metadata,
        target: PathBuf,
    },
}

impl Way {
    fn start(&self) -> &Path {
        match self {
            Way::Whole { root, .. } => root,
            Way::Toward { start, .. } => start,
        }
    }

    /// Whether the walk can meet the fixed rule path `fixed`: under the start, and for
    /// a way toward a target, on the way down to it or at or below it.
    fn meets(&self, fixed: &Path) -> bool {
        match self {
            Way::Whole { root, .. } => fixed.starts_with(root),
            Way::Toward { start, target, .. } => {
                let on_the_way = target.starts_with(fixed) || fixed.starts_with(target);
                fixed.starts_with(start) && on_the_way
            }
        }
    }
}
