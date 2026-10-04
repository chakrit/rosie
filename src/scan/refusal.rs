//! Screening every found item through the context's own delete checks, in every scan
//! mode: tree, caches, app, and orphans. The context decides; a scan only turns its
//! verdict into a plan entry (`docs/spec/safety.md#running-processes`, `#roots`):
//!
//! - an item the checks pass is planned inside the roots;
//! - one outside the roots is planned blocked, with its `rosie roots add` hint;
//! - one a process runs a program from is left out of the plan, sized, and reported;
//! - a mount point is a walk skip, never planned (`docs/spec/safety.md#walk-skips`);
//! - one the checks refuse otherwise is a scan problem.
//!
//! A refused item is sized too, for the end-of-run stats, by a sizer of its own: a file
//! it shares with a planned item still weighs in the plan, and its bytes stay out of the
//! scan's progress.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rayon::Scope;

use super::entry::Entry;
use super::log::{Log, SizingLog};
use super::progress::NoProgress;
use super::sizing::{Measure, Sizer};
use crate::fs::{self, Backend, Cleanup, FsContext};
use crate::plan::{Reach, Size, WalkSkip};

/// What the context's delete checks say about a found item.
#[derive(Debug)]
pub(crate) enum Verdict {
    Planned(Reach),
    Running {
        pid: u32,
        program: PathBuf,
    },
    /// Never planned, whatever the settings; logged as a walk skip.
    Skipped(WalkSkip),
    Problem(fs::Error),
}

/// Runs the delete checks on a found item without deleting it.
pub(crate) fn screen<B: Backend>(ctx: &FsContext<B>, path: &Path) -> Verdict {
    match Cleanup::delete(path).verify(ctx) {
        Ok(()) => Verdict::Planned(Reach::InRoots),
        Err(fs::Error::OutsideRoots { .. }) => Verdict::Planned(Reach::OutsideRoots),
        Err(fs::Error::Running { pid, program, .. }) => Verdict::Running { pid, program },
        Err(fs::Error::CrossesVolume { .. }) => Verdict::Skipped(WalkSkip::SealedMount),
        Err(error) => Verdict::Problem(error),
    }
}

/// An item left out of the plan because a process runs a program from inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub path: PathBuf,
    /// Every rule that matched the path, sorted.
    pub rules: Vec<String>,
    pub pid: u32,
    /// The program file the process runs, inside the path.
    pub program: PathBuf,
    /// Sized apart from the plan, for the stats; its hardlinks are counted once among
    /// the refused items and do not take bytes from a planned one.
    pub size: Size,
}

/// Sizes the refused items of one scan, from any of its workers.
pub(crate) struct RefusedItems<'a, B: Backend> {
    sizer: Sizer<'a, B, NoProgress>,
    refused: Mutex<Vec<(Pending, Measure)>>,
}

/// A refused item before its size is final.
struct Pending {
    path: PathBuf,
    rules: Vec<String>,
    pid: u32,
    program: PathBuf,
}

impl<'a, B: Backend + Sync> RefusedItems<'a, B> {
    pub fn new(ctx: &'a FsContext<B>, log: &'a Log) -> Self {
        RefusedItems {
            sizer: Sizer::new(ctx, &NoProgress, SizingLog::refused(log)),
            refused: Mutex::default(),
        }
    }

    /// Records `entry` as refused for the process `pid` running `program`, under the
    /// names `rules` gives, and sizes it on `scope`.
    pub fn refuse<'s>(
        &'s self,
        scope: &Scope<'s>,
        entry: Entry,
        rules: Vec<String>,
        pid: u32,
        program: PathBuf,
    ) {
        let pending = Pending {
            path: entry.path().to_path_buf(),
            rules: sorted(rules),
            pid,
            program,
        };
        let size = self.sizer.measure(scope, entry);
        self.lock().push((pending, size));
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
                pid: pending.pid,
                program: pending.program,
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::fs::fake::{FakeBackend, USER_UID};
    use crate::fs::{Bounds, Paths};

    fn ctx(fake: &FakeBackend) -> FsContext<&FakeBackend> {
        let bounds = Bounds {
            roots: vec![PathBuf::from("/Users/me")],
            paths: Paths::new(Path::new("/Users/me")).expect("absolute home"),
            user_uid: USER_UID,
        };
        FsContext::new(fake, bounds).expect("absolute bounds")
    }

    /// A mount point is never planned and is not a scan problem
    /// (`docs/spec/safety.md#walk-skips`).
    #[test]
    fn a_matched_mount_point_is_a_mount_skip() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/.npm");
        fake.mount("/Users/me/.npm", 9);

        let verdict = screen(&ctx(&fake), Path::new("/Users/me/.npm"));

        assert!(
            matches!(verdict, Verdict::Skipped(WalkSkip::SealedMount)),
            "got {verdict:?}"
        );
    }
}
