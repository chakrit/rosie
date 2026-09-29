//! Claiming found targets: the running-process refusal, the item marks, and the start
//! of sizing (`docs/spec/safety.md#running-processes`, `#roots`, `#elevation`).

use std::sync::{Mutex, MutexGuard};

use rayon::Scope;

use super::entry::Entry;
use super::log::{Log, Problem, SizingLog};
use super::progress::Progress;
use super::refusal::{Refusals, Refused};
use super::sizing::{Measure, Sizer};
use super::{RuleMatch, plan_twin};
use crate::fs::{Backend, Gate};
use crate::plan::{ItemKind, PathMatch, Reach, RunAs};
use crate::process::{IdlePath, ProcessTable};

/// Takes every target the walk or the fixed paths find, from any worker.
pub(super) struct Targets<'a, B: Backend, P: Progress> {
    gate: &'a Gate<B>,
    refusals: Refusals<'a, B>,
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
    path: IdlePath,
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
            twin: plan_twin(matched.tier),
        })
    }
}

impl<'a, B: Backend + Sync, P: Progress> Targets<'a, B, P> {
    pub fn new(gate: &'a Gate<B>, table: &'a ProcessTable, log: &'a Log, progress: &'a P) -> Self {
        Targets {
            gate,
            refusals: Refusals::new(gate, table, log),
            sizer: Sizer::new(gate, progress, SizingLog::walked(log)),
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
        let Some((entry, path)) = self.refusals.admit(scope, entry, names) else {
            return;
        };
        let Some(kind) = entry.item_kind() else {
            let path = path.into_path();
            return self.log.problem(Problem::Special { path });
        };
        let reach = match self.gate.within_roots(path.as_path()) {
            Ok(true) => Reach::InRoots,
            Ok(false) => Reach::OutsideRoots,
            Err(error) => return self.log.problem(Problem::Path(error)),
        };
        let run_as = match entry.meta().uid == self.gate.user_uid() {
            true => RunAs::User,
            false => RunAs::Sudo,
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
            refused: self.refusals.finish(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Target>> {
        self.claimed
            .lock()
            .expect("no scan worker panics while claiming")
    }
}
