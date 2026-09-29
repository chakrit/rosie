//! Sizing found targets in parallel with the walk (`docs/spec/performance.md#scan`).
//!
//! Sizes are allocated bytes. A file with more than one name is counted once per
//! sizer, by dev and inode, so a file hardlinked into two targets, or twice into one,
//! weighs once in the plan (`docs/spec/plan.md#stats`); only such files enter the
//! shared set of counted entries. Sizing descends into bundles, since deleting a target
//! removes them too, but never into another volume or a dataless placeholder, which
//! are neither counted nor opened; a denied folder is a walk skip.
//!
//! Each folder's entries are summed on the worker listing it, and the sum is added to
//! the target's total and reported as progress once per folder, so workers share no
//! counter per entry.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rayon::Scope;

use super::entry::Entry;
use super::log::{Log, SizingLog};
use super::progress::Progress;
use crate::fs::{Backend, FileKind, Gate, Metadata};
use crate::plan::{Size, WalkSkip};

/// Sizes targets against one shared set of counted entries.
pub struct Sizer<'a, B: Backend, P: Progress> {
    gate: &'a Gate<B>,
    progress: &'a P,
    log: SizingLog<'a>,
    counted: InodeSet,
}

/// The running total of one target, final once the sizing scope has ended.
#[derive(Debug, Clone, Default)]
pub struct Measure(Arc<AtomicU64>);

impl Measure {
    pub fn get(&self) -> Size {
        Size::bytes(self.0.load(Ordering::Relaxed))
    }

    fn add(&self, bytes: u64) {
        let saturating = |total: u64| Some(total.saturating_add(bytes));
        let updated = self
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, saturating);
        if let Err(total) = updated {
            unreachable!("a saturating add always yields a total, yet {total} was kept");
        }
    }
}

impl<'a, B: Backend + Sync, P: Progress> Sizer<'a, B, P> {
    /// Builds a sizer for a target the caller has entered and claimed: what it meets
    /// while sizing counts as part of that scan's walk (`docs/spec/safety.md#walk-skips`).
    /// This is the only way to build a [`Sizer`] from outside `scan`; the refused-target
    /// sink stays internal to the scan.
    pub fn walking(gate: &'a Gate<B>, progress: &'a P, log: &'a Log) -> Self {
        Self::new(gate, progress, SizingLog::walked(log))
    }

    pub(super) fn new(gate: &'a Gate<B>, progress: &'a P, log: SizingLog<'a>) -> Self {
        Sizer {
            gate,
            progress,
            log,
            counted: InodeSet::new(),
        }
    }

    /// Starts sizing `entry` on `scope`.
    pub fn measure<'s>(&'s self, scope: &Scope<'s>, entry: Entry) -> Measure {
        let meta = entry.meta();
        let total = Measure::default();
        self.publish(&total, self.weight(meta));

        if meta.kind == FileKind::Dir {
            let (folder, folder_total) = (entry.into_path(), total.clone());
            scope.spawn(move |scope| self.size_folder(scope, folder, meta.dev, folder_total));
        }
        total
    }

    /// Adds the weights of the folder's entries to `total` in one step, and starts
    /// sizing each subfolder on its own task.
    fn size_folder<'s>(&'s self, scope: &Scope<'s>, folder: PathBuf, dev: u64, total: Measure) {
        let names = match self.gate.read_dir(&folder) {
            Ok(names) => names,
            Err(error) => return self.log.entry_failed(folder, error),
        };

        let mut sum = 0u64;
        for name in names {
            let path = folder.join(name);
            let meta = match self.gate.lstat(&path) {
                Ok(meta) => meta,
                Err(error) => {
                    self.log.entry_failed(path, error);
                    continue;
                }
            };

            if meta.dev != dev {
                self.log.skip(path, WalkSkip::Mount);
                continue;
            }
            if meta.is_dataless() {
                self.log.skip(path, WalkSkip::Placeholder);
                continue;
            }

            sum = sum.saturating_add(self.weight(meta));
            if meta.kind == FileKind::Dir {
                let total = total.clone();
                scope.spawn(move |scope| self.size_folder(scope, path, dev, total));
            }
        }
        self.publish(&total, sum);
    }

    /// The entry's allocated bytes, or nothing when another of its names was counted
    /// already. APFS gives folders no second name, so only files are looked up.
    fn weight(&self, meta: Metadata) -> u64 {
        let named_once = meta.kind == FileKind::Dir || meta.nlink <= 1;
        let first_sight = named_once || self.counted.claim(meta.dev, meta.inode);
        match first_sight {
            true => meta.allocated,
            false => 0,
        }
    }

    fn publish(&self, total: &Measure, bytes: u64) {
        total.add(bytes);
        self.progress.sized(bytes);
    }
}

/// How many locks the counted set is split over, so workers rarely wait on each other.
const SHARDS: usize = 64;

/// One shard of the counted set: (dev, inode) pairs.
type Shard = Mutex<HashSet<(u64, u64)>>;

/// A set of (dev, inode) pairs of files with more than one name, shared by every
/// worker.
struct InodeSet {
    shards: Box<[Shard]>,
}

impl InodeSet {
    fn new() -> Self {
        let shards = (0..SHARDS).map(|_| Mutex::default()).collect();
        InodeSet { shards }
    }

    /// Adds the pair; true when it was not in the set yet.
    fn claim(&self, dev: u64, inode: u64) -> bool {
        let shard = (inode ^ dev.rotate_left(32)) as usize % SHARDS;
        self.shards[shard]
            .lock()
            .expect("no scan worker panics while sizing")
            .insert((dev, inode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_inode_number_on_two_volumes_is_two_files() {
        let counted = InodeSet::new();

        assert!(counted.claim(1, 7));
        assert!(counted.claim(2, 7));
        assert!(!counted.claim(1, 7));
    }
}
