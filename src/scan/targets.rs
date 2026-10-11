//! Claiming found targets: each is screened by [`Screening`], which gives its marks,
//! and the admitted are counted as found and start sizing.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use rayon::Scope;

use super::RuleMatch;
use super::entry::Entry;
use super::log::{Log, Problem, SizingLog};
use super::progress::Progress;
use super::refusal::{Admitted, Refused, Screening};
use super::sizing::{Measure, Sizer};
use crate::fs::{Backend, FsContext};
use crate::plan::{ItemKind, PathMatch, Reach, RunAs};

/// Takes every target the walk or the fixed paths find, from any worker.
pub(super) struct Targets<'a, B: Backend, P: Progress> {
    screening: Screening<'a, B>,
    sizer: Sizer<'a, B, P>,
    log: &'a Log,
    progress: &'a P,
    claimed: Mutex<Vec<Target>>,
}

/// Everything claimed, once the scan's workers are done.
#[derive(Debug)]
pub(super) struct Found {
    pub targets: Vec<Target>,
    pub refused: Vec<Refused>,
}

/// A path to delete, with its marks and its running size.
#[derive(Debug)]
pub(super) struct Target {
    path: PathBuf,
    kind: ItemKind,
    rules: Vec<RuleMatch>,
    run_as: RunAs,
    reach: Reach,
    size: Measure,
}

impl Target {
    /// One plan match per rule that accepted the target.
    pub fn path_matches(&self) -> impl Iterator<Item = PathMatch> + '_ {
        self.rules.iter().map(|matched| PathMatch {
            path: self.path.clone(),
            kind: self.kind,
            size: self.size.get(),
            run_as: self.run_as,
            reach: self.reach,
            rule: matched.rule.to_string(),
            tier: matched.tier,
        })
    }
}

impl<'a, B: Backend + Sync, P: Progress> Targets<'a, B, P> {
    pub fn new(ctx: &'a FsContext<B>, log: &'a Log, progress: &'a P) -> Self {
        Targets {
            screening: Screening::new(ctx, log),
            sizer: Sizer::new(ctx, progress, SizingLog::walked(log)),
            log,
            progress,
            claimed: Mutex::default(),
        }
    }

    /// Claims `entry` for the rules that matched it, and starts sizing it on `scope`.
    pub fn claim<'s>(&'s self, scope: &Scope<'s>, entry: Entry, rules: Vec<RuleMatch>) {
        let names = || {
            rules
                .iter()
                .map(|matched| matched.rule.to_string())
                .collect()
        };
        let Some(Admitted {
            entry,
            reach,
            run_as,
        }) = self.screening.admit(scope, entry, names)
        else {
            return;
        };
        let path = entry.path().to_path_buf();
        let Some(kind) = entry.item_kind() else {
            return self.log.problem(Problem::Special { path });
        };

        self.progress.found();
        let size = self.sizer.measure(scope, entry);

        let target = Target {
            path,
            kind,
            rules,
            run_as,
            reach,
            size,
        };
        self.lock().push(target);
    }

    /// What was claimed; called once the scope the sizing ran on has ended.
    pub fn finish(self) -> Found {
        let targets = self
            .claimed
            .into_inner()
            .expect("no scan worker panics while claiming");
        Found {
            targets,
            refused: self.screening.finish(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Target>> {
        self.claimed
            .lock()
            .expect("no scan worker panics while claiming")
    }
}
