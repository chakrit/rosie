//! Screening every found item through the context's own delete checks, in every scan
//! mode: tree, caches, app, and orphans. The context decides; a scan only turns its
//! verdict into a plan entry (`docs/spec/safety.md#running-processes`, `#roots`,
//! `#elevation`):
//!
//! - an item the checks pass is planned inside the roots;
//! - one outside the roots is planned `skip`, with its `rosie roots add` hint;
//! - one a process runs a program from is left out of the plan, sized, and reported;
//! - a mount point is a walk skip, never planned (`docs/spec/safety.md#walk-skips`);
//! - one the checks refuse otherwise is a scan problem.
//!
//! A planned item is marked `sudo` when the user does not own it.
//!
//! A refused item is sized too, for the end-of-run stats, by a sizer of its own: a file
//! it shares with a planned item still weighs in the plan, and its bytes stay out of the
//! scan's progress.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rayon::Scope;

use super::entry::Entry;
use super::log::{Log, Problem, SizingLog};
use super::progress::NoProgress;
use super::sizing::{Measure, Sizer};
use super::skip::WalkSkip;
use crate::fs::{self, Backend, Cleanup, FsContext};
use crate::plan::{Reach, RunAs, Size};

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

/// Screens the items one scan found, from any of its workers, and sizes the refused.
pub(crate) struct Screening<'a, B: Backend> {
    ctx: &'a FsContext<B>,
    log: &'a Log,
    refused_sizer: Sizer<'a, B, NoProgress>,
    refused: Mutex<Vec<(Pending, Measure)>>,
}

/// A found item the delete checks let into the plan, with the marks they give it.
pub(crate) struct Admitted {
    pub entry: Entry,
    pub reach: Reach,
    pub run_as: RunAs,
}

/// A refused item before its size is final.
struct Pending {
    path: PathBuf,
    rules: Vec<String>,
    pid: u32,
    program: PathBuf,
}

/// What the context's delete checks say about a found item.
#[derive(Debug)]
enum Verdict {
    Planned(Reach),
    Running {
        pid: u32,
        program: PathBuf,
    },
    /// Never planned, whatever the settings; logged as a walk skip.
    Skipped(WalkSkip),
    Problem(fs::Error),
}

impl<'a, B: Backend + Sync> Screening<'a, B> {
    pub fn new(ctx: &'a FsContext<B>, log: &'a Log) -> Self {
        Screening {
            ctx,
            log,
            refused_sizer: Sizer::new(ctx, &NoProgress, SizingLog::refused(log)),
            refused: Mutex::default(),
        }
    }

    /// Runs the delete checks on `entry` and admits it when they plan it. Otherwise the
    /// entry is refused and sized on `scope` under the names `rule_names` gives, logged
    /// as a walk skip, or logged as a problem, and nothing is admitted.
    pub fn admit<'s>(
        &'s self,
        scope: &Scope<'s>,
        entry: Entry,
        rule_names: impl FnOnce() -> Vec<String>,
    ) -> Option<Admitted> {
        let reach = match screen(self.ctx, entry.path()) {
            Verdict::Planned(reach) => reach,
            Verdict::Running { pid, program } => {
                self.refuse(scope, entry, rule_names(), pid, program);
                return None;
            }
            Verdict::Skipped(reason) => {
                self.log.skip(entry.path().to_path_buf(), reason);
                return None;
            }
            Verdict::Problem(error) => {
                let met_at = entry.path().to_path_buf();
                self.log.problem(Problem::Path { met_at, error });
                return None;
            }
        };

        let run_as = match entry.meta().uid == self.ctx.user_uid() {
            true => RunAs::User,
            false => RunAs::Sudo,
        };
        Some(Admitted {
            entry,
            reach,
            run_as,
        })
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

    /// Records `entry` as refused for the process `pid` running `program`, under the
    /// names `rules`, and sizes it on `scope`.
    fn refuse<'s>(
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
        let size = self.refused_sizer.measure(scope, entry);
        self.lock().push((pending, size));
    }

    fn lock(&self) -> MutexGuard<'_, Vec<(Pending, Measure)>> {
        self.refused
            .lock()
            .expect("no scan worker panics while refusing")
    }
}

/// Runs the delete checks on a found item without deleting it.
fn screen<B: Backend>(ctx: &FsContext<B>, path: &Path) -> Verdict {
    match Cleanup::delete(path).verify(ctx) {
        Ok(()) => Verdict::Planned(Reach::InRoots),
        Err(fs::Error::OutsideRoots { .. }) => Verdict::Planned(Reach::OutsideRoots),
        Err(fs::Error::Running { pid, program, .. }) => Verdict::Running { pid, program },
        Err(fs::Error::CrossesVolume { .. }) => Verdict::Skipped(WalkSkip::SealedMount),
        Err(error) => Verdict::Problem(error),
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
