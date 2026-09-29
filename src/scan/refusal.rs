//! The running-process refusal every scan mode passes its found items through
//! (`docs/spec/safety.md#running-processes`): tree, caches, app, and orphans.
//!
//! A refused item is sized too, for the end-of-run stats, by a sizer of its own: a file
//! it shares with a planned item still weighs in the plan, and its bytes stay out of the
//! scan's progress.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use rayon::Scope;

use super::entry::Entry;
use super::log::{Log, SizingLog};
use super::progress::NoProgress;
use super::sizing::{Measure, Sizer};
use crate::fs::{Backend, Gate};
use crate::plan::Size;
use crate::process::{Admission, IdlePath, Process, ProcessTable};

/// An item left out of the plan because a process executes from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub path: PathBuf,
    /// Every rule that matched the path, sorted.
    pub rules: Vec<String>,
    pub process: Process,
    /// Sized apart from the plan, for the stats; its hardlinks are counted once among
    /// the refused items and do not take bytes from a planned one.
    pub size: Size,
}

/// Admits found items into a plan, or refuses them, from any worker of one scan.
pub(crate) struct Refusals<'a, B: Backend> {
    table: &'a ProcessTable,
    sizer: Sizer<'a, B, NoProgress>,
    refused: Mutex<Vec<(Pending, Measure)>>,
}

/// A refused item before its size is final.
struct Pending {
    path: PathBuf,
    rules: Vec<String>,
    process: Process,
}

impl<'a, B: Backend + Sync> Refusals<'a, B> {
    pub fn new(gate: &'a Gate<B>, table: &'a ProcessTable, log: &'a Log) -> Self {
        Refusals {
            table,
            sizer: Sizer::new(gate, &NoProgress, SizingLog::refused(log)),
            refused: Mutex::default(),
        }
    }

    /// The entry with its path admitted, when no process executes from it. Otherwise
    /// the entry is refused under the names `rules` gives, sized on `scope`, and
    /// nothing is returned.
    pub fn admit<'s>(
        &'s self,
        scope: &Scope<'s>,
        entry: Entry,
        rules: impl FnOnce() -> Vec<String>,
    ) -> Option<(Entry, IdlePath)> {
        match self.table.admit(entry.path().to_path_buf()) {
            Admission::Idle(path) => Some((entry, path)),
            Admission::Running { path, process } => {
                let pending = Pending {
                    path,
                    rules: sorted(rules()),
                    process: process.clone(),
                };
                let size = self.sizer.measure(scope, entry);
                self.lock().push((pending, size));
                None
            }
        }
    }

    /// Every refused item, sorted by path; called once the scope the sizing ran on has
    /// ended.
    pub fn finish(self) -> Vec<Refused> {
        let refused = self
            .refused
            .into_inner()
            .expect("no scan worker panics while refusing");
        let mut refused: Vec<Refused> = refused
            .into_iter()
            .map(|(pending, size)| Refused {
                path: pending.path,
                rules: pending.rules,
                process: pending.process,
                size: size.get(),
            })
            .collect();
        refused.sort_by(|a, b| a.path.cmp(&b.path));
        refused
    }

    fn lock(&self) -> MutexGuard<'_, Vec<(Pending, Measure)>> {
        self.refused
            .lock()
            .expect("no scan worker panics while refusing")
    }
}

fn sorted(mut rules: Vec<String>) -> Vec<String> {
    rules.sort();
    rules.dedup();
    rules
}
