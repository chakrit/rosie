//! Claiming found targets: the running-process refusal, the item marks, and the start
//! of sizing (`docs/spec/safety.md#running-processes`, `#roots`, `#elevation`).
//!
//! Refused targets are sized too, for the end-of-run stats, by a sizer of their own:
//! a file they share with a planned target still weighs in the plan, and their bytes
//! stay out of the scan's progress.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use rayon::Scope;

use super::entry::Entry;
use super::log::{Log, Problem, SizingLog};
use super::progress::{NoProgress, Progress};
use super::sizing::{Measure, Sizer};
use super::{Refused, RuleMatch, plan_twin};
use crate::fs::{Backend, FileKind, Gate};
use crate::plan::{ItemKind, PathMatch, Reach, RunAs};
use crate::process::{Process, ProcessTable};

/// Takes every target the walk or the fixed paths find, from any worker.
pub(super) struct Targets<'a, B: Backend, P: Progress> {
    gate: &'a Gate<B>,
    table: &'a ProcessTable,
    sizer: Sizer<'a, B, P>,
    refused_sizer: Sizer<'a, B, NoProgress>,
    log: &'a Log,
    progress: &'a P,
    claimed: Mutex<Claimed>,
}

/// Everything claimed, once the scan's workers are done.
#[derive(Debug)]
pub(super) struct Found {
    pub targets: Vec<Target>,
    pub refused: Vec<Refused>,
}

/// What the workers have claimed so far; sizes are final once they are done.
#[derive(Debug, Default)]
struct Claimed {
    targets: Vec<Target>,
    refused: Vec<(Refusal, Measure)>,
}

/// A refused target before its size is final.
#[derive(Debug)]
struct Refusal {
    path: PathBuf,
    rules: Vec<String>,
    process: Process,
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
            twin: plan_twin(matched.tier),
        })
    }
}

impl<'a, B: Backend + Sync, P: Progress> Targets<'a, B, P> {
    pub fn new(gate: &'a Gate<B>, table: &'a ProcessTable, log: &'a Log, progress: &'a P) -> Self {
        Targets {
            gate,
            table,
            sizer: Sizer::new(gate, progress, SizingLog::walked(log)),
            refused_sizer: Sizer::new(gate, &NoProgress, SizingLog::refused(log)),
            log,
            progress,
            claimed: Mutex::default(),
        }
    }

    /// Claims `entry` for the rules that matched it, and starts sizing it on `scope`.
    pub fn claim<'s>(&'s self, scope: &Scope<'s>, entry: Entry, rules: Vec<RuleMatch>) {
        let (path, meta) = (entry.path().to_path_buf(), entry.meta());
        if let Some(process) = self.table.executing_from(&path) {
            let size = self.refused_sizer.measure(scope, entry);
            let refusal = Refusal {
                path,
                rules: rule_names(&rules),
                process: process.clone(),
            };
            return self.lock().refused.push((refusal, size));
        }
        let Some(kind) = item_kind(meta.kind) else {
            return self.log.problem(Problem::Special { path });
        };
        let reach = match self.gate.within_roots(&path) {
            Ok(true) => Reach::InRoots,
            Ok(false) => Reach::OutsideRoots,
            Err(error) => return self.log.problem(Problem::Path(error)),
        };
        let run_as = match meta.uid == self.gate.user_uid() {
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
        self.lock().targets.push(target);
    }

    /// What was claimed; called once the scope the sizing ran on has ended.
    pub fn finish(self) -> Found {
        let claimed = self
            .claimed
            .into_inner()
            .expect("no scan worker panics while claiming");
        let refused = claimed
            .refused
            .into_iter()
            .map(|(refusal, size)| Refused {
                path: refusal.path,
                rules: refusal.rules,
                process: refusal.process,
                size: size.get(),
            })
            .collect();
        Found {
            targets: claimed.targets,
            refused,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Claimed> {
        self.claimed
            .lock()
            .expect("no scan worker panics while claiming")
    }
}

/// The matched rules' names, sorted. A target lists each rule once.
fn rule_names(rules: &[RuleMatch]) -> Vec<String> {
    let mut names: Vec<String> = rules
        .iter()
        .map(|matched| matched.rule.to_string())
        .collect();
    names.sort();
    names
}

/// What a plan entry records of a matched entry's type. Symlinks never reach here;
/// sockets, fifos, and device nodes are never planned.
fn item_kind(kind: FileKind) -> Option<ItemKind> {
    match kind {
        FileKind::File => Some(ItemKind::File),
        FileKind::Dir => Some(ItemKind::Folder),
        FileKind::Symlink | FileKind::Other => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_files_and_folders_are_plan_items() {
        assert_eq!(item_kind(FileKind::File), Some(ItemKind::File));
        assert_eq!(item_kind(FileKind::Dir), Some(ItemKind::Folder));
        assert_eq!(item_kind(FileKind::Other), None);
        assert_eq!(item_kind(FileKind::Symlink), None);
    }
}
