//! Scanning: finding what the rules match and building the plan from it
//! (`docs/spec/cli.md#modes`, `docs/spec/performance.md#scan`).
//!
//! - `tree <dir>` walks the folder in parallel, applying the folder rules and the fixed
//!   rule paths that lie under it. The walk stops descending at a match, never enters or
//!   matches a symlink, and prunes the walk skips (`docs/spec/safety.md#walk-skips`).
//! - `caches` takes the fixed rule paths and the tool rules.
//!
//! Every target is sized as soon as it is found, while the walk goes on, and is marked
//! blocked when outside the roots and `sudo` when not owned by the user. Targets a
//! running process executes from are refused and listed apart from the plan.

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
use crate::fs::{self, Backend, FileKind, Gate, Home};
use crate::plan::{self, AggressiveItems, Needs, Plan, PlanBuilder, ToolCmds, WalkSkip, WalkSkips};
use crate::process::{self, ProcessTable};
use crate::rules::{RuleId, RuleSet, Shape, Tier, Twin};

pub use entry::{Entry, Folder, InFolder, Mounts, NotAFolder, NotAnEntry, Sealed};
pub use log::{Log, Problem, Skipped};
pub use progress::{NoProgress, Progress, StderrProgress};
pub(crate) use refusal::Refusals;
pub use refusal::Refused;
pub use sizing::{Measure, Sizer};

use fixed::{FixedClaim, FixedTargets};
use targets::Targets;
use walk::TreeWalk;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error("{path} is not a folder", path = path.display())]
    NotAFolder { path: PathBuf },

    #[error(transparent)]
    Process(#[from] process::Error),
}

/// What a scan is told beyond the rules: the injected home for `~`, the walk flags
/// (config `[walk]` with the CLI flags applied), and whether `--aggressive` was given.
#[derive(Debug, Clone)]
pub struct Settings {
    pub home: Home,
    pub walk: Walk,
    pub aggressive: AggressiveItems,
}

/// What one scan found, in any mode: tree and caches here, app and orphans in
/// [`crate::apps`].
#[derive(Debug)]
pub struct Scan {
    pub plan: Plan,
    /// Items a running process executes from, kept out of the plan, sorted by path.
    pub refused: Vec<Refused>,
    /// Paths the walk or the sizing did not enter, sorted by path.
    pub skipped: Vec<Skipped>,
    /// Errors that cost the scan an item or a rule check, such as a fixed rule path
    /// through a symlink or a failing Lua rule. The rest of the scan went on.
    pub problems: Vec<Problem>,
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
    gate: Gate<B>,
    rules: RuleSet,
    settings: Settings,
}

impl<B: Backend + Sync> Scanner<B> {
    pub fn new(gate: Gate<B>, rules: RuleSet, settings: Settings) -> Self {
        Scanner {
            gate,
            rules,
            settings,
        }
    }

    /// Scans `dir`, a path the user typed: refused when it passes through a symlink or
    /// is spelled differently from the disk. The folder itself is never a target; the
    /// walk looks at everything below it. A folder that is a dataless placeholder is
    /// not opened unless the walk may enter placeholders.
    pub fn tree<P: Progress>(&self, dir: &Path, progress: &P) -> Result<Scan, Error> {
        let root = self.gate.check_typed_path(dir)?;
        let root_meta = self.gate.lstat(&root)?;
        if root_meta.kind != FileKind::Dir {
            return Err(Error::NotAFolder { path: root });
        }
        let table = ProcessTable::query(&self.gate)?;
        let log = Log::default();
        let fixed = FixedTargets::of(&self.rules, &self.settings.home).under(&root);
        fixed.report_symlinked(&self.gate, &log);
        let fixed = fixed.into_map();

        let targets = Targets::new(&self.gate, &table, &log, progress);
        let walk = TreeWalk {
            gate: &self.gate,
            rules: &self.rules,
            fixed: &fixed,
            flags: self.settings.walk,
            targets: &targets,
            log: &log,
        };
        match root_meta.is_dataless() && !self.settings.walk.enter_placeholders {
            true => log.skip(root, WalkSkip::Closed(Needs::PLACEHOLDERS)),
            false => rayon::scope(|scope| walk.visit(scope, root, root_meta)),
        }

        let found = targets.finish();
        let builder = PlanBuilder::new(self.settings.aggressive);
        Ok(self.assemble(builder, found, log))
    }

    /// Scans the fixed rule paths, with `~` taken as the injected home, and lists the
    /// tool rules' commands. An absent path is not planned.
    pub fn caches<P: Progress>(&self, progress: &P) -> Result<Scan, Error> {
        let table = ProcessTable::query(&self.gate)?;
        let log = Log::default();
        let fixed = FixedTargets::of(&self.rules, &self.settings.home).outermost();

        let targets = Targets::new(&self.gate, &table, &log, progress);
        let claim = FixedClaim {
            gate: &self.gate,
            home: &self.settings.home,
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
        self.add_tools(&mut builder, &log);
        Ok(self.assemble(builder, found, log))
    }

    // plan

    fn add_tools(&self, builder: &mut PlanBuilder, log: &Log) {
        for rule in self.rules.iter() {
            let Shape::Tool(argvs) = rule.shape() else {
                continue;
            };
            let cmds = match argvs {
                Twin::Normal(cmd) => ToolCmds::Normal(cmd),
                Twin::Aggressive(cmd) => ToolCmds::Aggressive(cmd),
                Twin::Both { normal, aggressive } => ToolCmds::Twins {
                    cmd: normal,
                    cmd_aggressive: aggressive,
                },
            };

            let id = rule.id().to_string();
            if let Err(error) = builder.add_tool(&id, cmds) {
                log.problem(Problem::Plan(error));
            }
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
        Scan {
            plan: builder.build(),
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
