//! Sizing found targets in parallel with the walk (`docs/spec/performance.md#scan`).
//!
//! Sizes are allocated bytes. A target's first [`EXACT_FILES`] files are each sized;
//! past them, one file in [`SAMPLE_EVERY`] is sized and counted for that many, so a
//! huge target costs a fraction of its `lstat`s and its size is an estimate. A file
//! with more than one name is counted once per sizer, by dev and inode, so a file
//! hardlinked into two targets, or twice into one, weighs once in the plan
//! (`docs/spec/plan.md#stats`); only such files enter the shared set of counted
//! entries. Sizing descends into bundles, since deleting a target
//! removes them too, but never into a mount root or a dataless placeholder it meets
//! inside a target, told by `Sealed::of`; those are neither counted nor opened. A
//! denied folder is a walk skip when the target was claimed, and is dropped for a
//! refused one.
//!
//! Each folder's entries are summed on the worker listing it, and the sum is added to
//! the target's total and reported as progress once per folder, so workers share no
//! counter per entry.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rayon::Scope;

use super::entry::{Entry, Sealed};
use super::log::{Log, SizingLog};
use super::progress::Progress;
use crate::fs::{Backend, Cleanup, FileKind, FsContext, FsOp, Metadata};
use crate::plan::Size;

/// Sizes targets against one shared set of counted entries.
pub struct Sizer<'a, B: Backend, P: Progress> {
    ctx: &'a FsContext<B>,
    progress: &'a P,
    log: SizingLog<'a>,
    counted: InodeSet,
}

/// A target's first files, counted in the order its folders are listed, are all sized.
const EXACT_FILES: u64 = 10_000;
/// Past [`EXACT_FILES`], one file in this many is sized and counted this many times.
const SAMPLE_EVERY: u64 = 8;

/// The running total of one target, final once the sizing scope has ended, and how
/// many of its files the sizing has met.
#[derive(Debug, Clone, Default)]
pub struct Measure(Arc<Tally>);

#[derive(Debug, Default)]
struct Tally {
    bytes: AtomicU64,
    files: AtomicU64,
}

impl Measure {
    pub fn get(&self) -> Size {
        Size::bytes(self.0.bytes.load(Ordering::Relaxed))
    }

    fn add(&self, bytes: u64) {
        let saturating = |total: u64| Some(total.saturating_add(bytes));
        let updated = self
            .0
            .bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, saturating);
        if let Err(total) = updated {
            unreachable!("a saturating add always yields a total, yet {total} was kept");
        }
    }

    /// Numbers the next `count` files of the target; the first one's number.
    fn number_files(&self, count: u64) -> u64 {
        self.0.files.fetch_add(count, Ordering::Relaxed)
    }
}

/// How many files the file numbered `number` in its target stands for when sized: one
/// within the first [`EXACT_FILES`], then [`SAMPLE_EVERY`] for one file in that many;
/// none for a file left unsized.
fn stands_for(number: u64) -> Option<u64> {
    match number < EXACT_FILES {
        true => Some(1),
        false => number.is_multiple_of(SAMPLE_EVERY).then_some(SAMPLE_EVERY),
    }
}

impl<'a, B: Backend + Sync, P: Progress> Sizer<'a, B, P> {
    /// Builds a sizer for a target the caller has entered and claimed: what it meets
    /// while sizing counts as part of that scan's walk. This is the only way to build a
    /// [`Sizer`] from outside `scan`; the refused-target sink stays internal to the scan.
    pub fn walking(ctx: &'a FsContext<B>, progress: &'a P, log: &'a Log) -> Self {
        Self::new(ctx, progress, SizingLog::walked(log))
    }

    pub(super) fn new(ctx: &'a FsContext<B>, progress: &'a P, log: SizingLog<'a>) -> Self {
        Sizer {
            ctx,
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
            scope.spawn(move |scope| self.size_folder(scope, folder, meta, folder_total));
        }
        total
    }

    /// Adds the weights of the folder's entries to `total` in one step, and starts
    /// sizing each subfolder on its own task. Every subfolder is `lstat`ed; the other
    /// entries are numbered within the target and sized as [`stands_for`] says. A sealed
    /// entry, told by [`Sealed::of`] against the folder's own `lstat`, is skipped. A
    /// symlink is weighed as a link, since a delete removes it as one.
    fn size_folder<'s>(
        &'s self,
        scope: &Scope<'s>,
        folder: PathBuf,
        folder_meta: Metadata,
        total: Measure,
    ) {
        let listed = match Cleanup::read_dir(&folder).perform(self.ctx) {
            Ok(listed) => listed,
            Err(error) => return self.log.entry_failed(folder, error),
        };

        let (folders, files): (Vec<_>, Vec<_>) = listed
            .into_iter()
            .partition(|entry| entry.kind == FileKind::Dir);
        let first_file = total.number_files(files.len() as u64);
        let sized_files = files
            .into_iter()
            .zip(first_file..)
            .filter_map(|(entry, number)| Some((entry, stands_for(number)?)));
        let sized = folders
            .into_iter()
            .map(|entry| (entry, 1))
            .chain(sized_files);

        let mut sum = 0u64;
        for (entry, stands_for) in sized {
            let path = folder.join(entry.name);
            let meta = match Cleanup::lstat(&path).perform(self.ctx) {
                Ok(meta) => meta,
                Err(error) => {
                    self.log.entry_failed(path, error);
                    continue;
                }
            };

            if let Some(sealed) = Sealed::of(meta, folder_meta) {
                self.log.skip(path, sealed.skip());
                continue;
            }

            sum = sum.saturating_add(self.weight(meta).saturating_mul(stands_for));
            if meta.kind == FileKind::Dir {
                let total = total.clone();
                scope.spawn(move |scope| self.size_folder(scope, path, meta, total));
            }
        }
        self.publish(&total, sum);
    }

    /// The entry's allocated bytes, or nothing when another of its names was counted
    /// already. APFS gives folders no second name, so folders are never looked up.
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

    #[test]
    fn past_the_exact_files_one_file_in_a_sample_stands_for_the_rest() {
        let counted: Vec<_> = [0, EXACT_FILES - 1, EXACT_FILES, EXACT_FILES + 1]
            .into_iter()
            .chain([EXACT_FILES + SAMPLE_EVERY])
            .map(stands_for)
            .collect();

        assert_eq!(
            counted,
            [
                Some(1),
                Some(1),
                Some(SAMPLE_EVERY),
                None,
                Some(SAMPLE_EVERY)
            ]
        );
    }
}
