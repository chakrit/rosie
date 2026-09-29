//! What a scan tells the user besides the plan: the paths it did not enter and the
//! errors that cost it an item or a rule check (`docs/spec/safety.md#walk-skips`).

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use thiserror::Error;

use crate::fs;
use crate::plan::{self, WalkSkip};
use crate::rules;

/// A path the scan did not enter, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: PathBuf,
    pub reason: WalkSkip,
}

#[derive(Debug, Error)]
pub enum Problem {
    /// A rule's check failed on a candidate, which then does not match.
    #[error(transparent)]
    Rule(rules::Error),

    /// A path could not be read, or was refused, such as a fixed rule path through a
    /// symlink.
    #[error(transparent)]
    Path(fs::Error),

    /// A found item the plan cannot hold.
    #[error(transparent)]
    Plan(plan::Error),

    #[error("{path} is neither a file nor a folder; rosie does not delete it", path = path.display())]
    Special { path: PathBuf },
}

/// Collects skips and problems from every worker of one scan.
#[derive(Debug, Default)]
pub struct Log {
    skipped: Mutex<Vec<Skipped>>,
    problems: Mutex<Vec<Problem>>,
}

impl Log {
    pub fn skip(&self, path: PathBuf, reason: WalkSkip) {
        lock(&self.skipped).push(Skipped { path, reason });
    }

    pub fn problem(&self, problem: Problem) {
        lock(&self.problems).push(problem);
    }

    /// Records a failed listing or `lstat` of a walked or fixed path. A denial is a walk
    /// skip; a path that is absent, or vanished since its folder was listed, has
    /// nothing to scan.
    pub fn entry_failed(&self, path: PathBuf, error: fs::Error) {
        match classify(error) {
            Failure::Denied => self.skip(path, WalkSkip::Denied),
            Failure::Absent => {}
            Failure::Problem(problem) => self.problem(problem),
        }
    }

    pub fn into_parts(self) -> (Vec<Skipped>, Vec<Problem>) {
        let skipped = self.skipped.into_inner();
        let problems = self.problems.into_inner();
        (
            skipped.expect("no scan worker panics while logging"),
            problems.expect("no scan worker panics while logging"),
        )
    }
}

/// Where a sizer reports what it meets inside a target. A sizer holds only this, never
/// the scan's [`Log`] itself, so whether a skip counts is decided once, by the target's
/// kind, and no sizing step can count a skip the walk did not make.
pub struct SizingLog<'a>(Sink<'a>);

enum Sink<'a> {
    /// A target the walk entered and claimed: what sizing meets is part of the walk.
    Walked(&'a Log),
    /// A target refused before sizing started: the walk never entered it, so a mount,
    /// placeholder, or denial met while sizing it is not a walk skip, and an absent
    /// path has nothing to scan. Any other error is still a problem, since sizing it was
    /// still attempted.
    Refused(&'a Log),
}

impl<'a> SizingLog<'a> {
    pub fn walked(log: &'a Log) -> Self {
        SizingLog(Sink::Walked(log))
    }

    pub fn refused(log: &'a Log) -> Self {
        SizingLog(Sink::Refused(log))
    }

    pub fn skip(&self, path: PathBuf, reason: WalkSkip) {
        match self.0 {
            Sink::Walked(log) => log.skip(path, reason),
            Sink::Refused(_) => {}
        }
    }

    /// Records a failed listing or `lstat` met while sizing.
    pub fn entry_failed(&self, path: PathBuf, error: fs::Error) {
        match self.0 {
            Sink::Walked(log) => log.entry_failed(path, error),
            Sink::Refused(log) => {
                if let Failure::Problem(problem) = classify(error) {
                    log.problem(problem);
                }
            }
        }
    }
}

/// What a failed listing or `lstat` means for the scan.
enum Failure {
    /// The path was denied; a walk that reached it treats this as a walk skip.
    Denied,
    /// The path, or a folder on its way, does not exist: nothing to scan.
    Absent,
    /// Any other error, always reported.
    Problem(Problem),
}

fn classify(error: fs::Error) -> Failure {
    let denied = error.is_permission_denied();
    let absent = error.has_vanished();
    match (denied, absent) {
        (true, _) => Failure::Denied,
        (false, true) => Failure::Absent,
        (false, false) => Failure::Problem(Problem::Path(error)),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().expect("no scan worker panics while logging")
}
