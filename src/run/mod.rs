//! Executing a plan (`docs/spec/plan.md#run`, `docs/spec/safety.md#elevation`).
//!
//! A run does the user's ticked entries first, then pipes the remaining `sudo` entries
//! to one elevated child, which runs them through the same code. Within each part:
//!
//! 1. every delete is verified or refused before anything runs, and a refused delete
//!    takes its bootouts with it: a stale one, and one the context refuses, such as one
//!    a process runs from (the context's snapshot of running programs is taken before
//!    a bootout could stop that process) or a path outside the roots. A bootout
//!    withheld this way is skipped, never failed, whatever its delete ends as;
//! 2. the bootouts of the verified deletes, one at a time, so no plist is deleted with
//!    its job loaded;
//! 3. the verified deletes, in parallel, each checked again by the context as it is
//!    performed and reported as it finishes; alongside them, the tool commands and
//!    receipts, one at a time.
//!
//! A run has up to three steps: the user's part; the elevated part; then, in the
//! user's process again, the user's deletes the elevated part only booted `system`
//! jobs out for. The elevated child's context acts as root, which deletes only through
//! folders no one but root can change, and never deletes a user's item.
//!
//! A failed item is reported and the run goes on.

mod elevate;
pub mod elevated;
mod header;
mod item;
mod outcomes;
mod report;
mod root;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;
use std::thread::{self, ScopedJoinHandle};

use rayon::prelude::*;

use crate::fs::{self, Acting, Argv, Backend, Cleanup, Exit, FsContext, FsOp};
use crate::plan::{Delete, ItemSize, Outcome, Plan, RunAs, RunStats, Runnable, SkipReason};
use report::Forwarding;

pub use elevate::Elevation;
pub use item::{ItemResult, Subject};
pub use report::{OutputWindow, PlainReporter, Reporter, TerminalReporter};
pub use root::{RootError, RootRefusal, refuse_root};

pub struct Runner<B: Backend> {
    ctx: FsContext<B>,
}

impl<B: Backend + Sync> Runner<B> {
    /// `ctx` holds the roots loaded from the user's `config.toml`, since a plan never
    /// carries roots, and acts as the user for the run itself or as root for the
    /// elevated child.
    pub fn new(ctx: FsContext<B>) -> Self {
        Runner { ctx }
    }

    /// Whom this process acts as.
    fn acting(&self) -> RunAs {
        match self.ctx.acting() {
            Acting::User => RunAs::User,
            Acting::Root => RunAs::Sudo,
        }
    }

    /// Runs the plan's ticked entries: the user's part here, then the `sudo` part in
    /// one elevated child, then here the user's deletes whose jobs the child booted out.
    pub fn run<R: Reporter>(
        &self,
        plan: &Plan,
        elevation: &Elevation,
        reporter: &mut R,
    ) -> RunStats {
        let user_part = plan.ticked_for(RunAs::User);
        let sudo_part = plan.ticked_for(RunAs::Sudo);

        let mut results = self.execute(&user_part, reporter);
        if !sudo_part.is_empty() {
            let elevated = self.elevate(&sudo_part, elevation, reporter);
            let after = self.delete_after_elevation(&sudo_part, &elevated, reporter);
            results.extend(elevated);
            results.extend(after);
        }

        let mut stats = RunStats::default();
        for result in results {
            stats.record(result.size, result.outcome);
        }
        stats
    }

    /// Runs the part of `plan` that this process executes, as whoever it acts as. Only
    /// [`Runner::run`] and the elevated child call it, each with one part of a run, and
    /// tests of the elevated child.
    fn execute<R: Reporter>(&self, plan: &Plan, reporter: &mut R) -> Vec<ItemResult> {
        self.execute_runnable(&plan.runnable_as(self.acting()), reporter)
    }

    fn execute_runnable<R: Reporter>(
        &self,
        runnable: &Runnable,
        reporter: &mut R,
    ) -> Vec<ItemResult> {
        let verdicts = self.verify_all(runnable);
        let mut results = refused(runnable, &verdicts);
        for result in &results {
            reporter.item_finished(result);
        }

        let may_run = |delete: &Delete| matches!(verdicts.get(delete.path.as_path()), Some(Ok(())));
        let bootouts = runnable
            .holders()
            .iter()
            .filter(|delete| may_run(delete))
            .flat_map(|delete| delete.bootouts.iter());
        results.extend(
            bootouts.map(|bootout| self.run_command(bootout.absolute_argv().into(), reporter)),
        );

        let commands: Vec<Argv> = runnable
            .tools()
            .iter()
            .map(|tool| tool.command.argv())
            .chain(
                runnable
                    .receipts()
                    .iter()
                    .map(|receipt| receipt.absolute_argv().into()),
            )
            .collect();
        let verified: Vec<&Delete> = runnable
            .deletes()
            .iter()
            .copied()
            .filter(|delete| may_run(delete))
            .collect();

        let (sender, events) = mpsc::channel();
        let (deletes, commands) = thread::scope(|scope| {
            let mut forwarding = Forwarding::new(sender);
            let deleting = forwarding.clone();
            let commands = scope.spawn(move || {
                commands
                    .into_iter()
                    .map(|argv| self.run_command(argv, &mut forwarding))
                    .collect::<Vec<_>>()
            });
            let deletes = scope.spawn(move || {
                verified
                    .into_par_iter()
                    .map_with(deleting, |reporter, delete| {
                        let result = self.delete(delete);
                        reporter.item_finished(&result);
                        result
                    })
                    .collect::<Vec<_>>()
            });

            for event in events {
                event.deliver(reporter);
            }
            (joined(deletes), joined(commands))
        });

        results.extend(commands);
        results.extend(deletes);
        results
    }

    // verification

    /// Decides, before any bootout runs, whether each delete of `runnable` may run:
    /// those this process deletes and those whose jobs it only boots out. A delete that
    /// may not run takes its bootouts with it, so no job is unloaded for a plist that
    /// stays.
    fn verify_all<'a>(&self, runnable: &Runnable<'a>) -> Verdicts<'a> {
        let entries: BTreeMap<&'a Path, &'a Delete> = runnable
            .holders()
            .iter()
            .chain(runnable.deletes())
            .map(|delete| (delete.path.as_path(), *delete))
            .collect();
        entries
            .into_iter()
            .map(|(path, delete)| (path, self.verify(delete)))
            .collect()
    }

    /// Says why one delete must not run: it is stale, or the context refuses it, such as
    /// for a process running from it, for the running processes being unknown, or for a
    /// path outside the roots. `Ok` when it may run. A delete this process only boots a
    /// job out for, the user's in the elevated child, is checked as the user's process
    /// will check it.
    fn verify(&self, delete: &Delete) -> Result<(), Refusal> {
        if let Some(reason) = self.staleness(delete) {
            return Err(Refusal::Skipped(SkipReason::Stale, reason));
        }

        let op = Cleanup::delete(&delete.path);
        let verified = match delete.run_as == self.acting() {
            true => op.verify(&self.ctx),
            false => op.verify_as_user(&self.ctx),
        };
        verified.map_err(refusal_of)
    }

    /// Why the entry no longer describes what is on disk: its path is missing, a folder
    /// above it is missing or no longer a folder, or its `lstat` type changed since the
    /// scan. `None` when it still does, or when `lstat` fails otherwise and the
    /// context's own checks report it.
    fn staleness(&self, delete: &Delete) -> Option<String> {
        match Cleanup::lstat(&delete.path).perform(&self.ctx) {
            Ok(meta) if delete.kind.matches(meta.kind) => None,
            Ok(meta) => Some(format!(
                "was a {} and is now a {}",
                delete.kind.label(),
                meta.kind.label()
            )),
            Err(error) if error.has_vanished() => Some("no longer exists".to_owned()),
            Err(_) => None,
        }
    }

    // deletes

    /// Deletes a verified entry; performing the delete checks it again.
    fn delete(&self, delete: &Delete) -> ItemResult {
        let (outcome, reason) = match Cleanup::delete(&delete.path).perform(&self.ctx) {
            Ok(()) => (Outcome::Done, String::new()),
            Err(error) => refusal_of(error).of_delete(),
        };
        path_result(delete, outcome, reason)
    }

    // commands

    /// Runs one command, never through a shell, showing its last 3 output lines while
    /// it runs and keeping them as its result.
    fn run_command<R: Reporter>(&self, argv: Argv, reporter: &mut R) -> ItemResult {
        reporter.command_started(&argv);

        let mut window = OutputWindow::default();
        let exit = self.ctx.run_streaming(&argv, |line| {
            window.push(line);
            reporter.command_output(&argv, &window);
        });
        let (outcome, reason) = match exit {
            Ok(exit) if exit.success() => (Outcome::Done, String::new()),
            Ok(exit) => (Outcome::Failed, exit_reason(exit)),
            Err(error) => (Outcome::Failed, error.to_string()),
        };

        let result = ItemResult {
            subject: Subject::Command {
                argv,
                output: window.into_lines(),
            },
            size: ItemSize::Unknown,
            outcome,
            reason,
        };
        reporter.item_finished(&result);
        result
    }
}

/// What a worker thread returned, or its panic, carried on in this thread.
fn joined<T>(worker: ScopedJoinHandle<'_, T>) -> T {
    worker
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

// refusals

/// Why a delete must not run.
#[derive(Debug, Clone)]
enum Refusal {
    /// The delete is skipped for this reason, and its bootouts with it.
    Skipped(SkipReason, String),
    /// The context refused the delete, which fails.
    Refused(String),
}

impl Refusal {
    /// What the refused delete ends as, and why.
    fn of_delete(self) -> (Outcome, String) {
        match self {
            Refusal::Skipped(skip, reason) => (Outcome::Skipped(skip), reason),
            Refusal::Refused(reason) => (Outcome::Failed, reason),
        }
    }

    /// Why each bootout of the refused delete is withheld. A withheld bootout never
    /// ran, so it is a skip whatever the delete ends as, and no one that reads it takes
    /// its job for booted out.
    fn of_bootout(self) -> (SkipReason, String) {
        match self {
            Refusal::Skipped(skip, reason) => (skip, reason),
            Refusal::Refused(reason) => (SkipReason::DeleteRefused, reason),
        }
    }
}

/// Each delete of a part, by path, verified or refused.
type Verdicts<'a> = BTreeMap<&'a Path, Result<(), Refusal>>;

/// Why the context refused a delete. One a process runs from, or one the running
/// processes could not be checked for (`docs/spec/safety.md#running-processes`), is
/// skipped with its bootouts; so is one root may not delete, because someone other
/// than root could change a folder on its way, for the user to delete by hand. Any
/// other refusal fails the delete.
fn refusal_of(error: fs::Error) -> Refusal {
    match error {
        error @ fs::Error::Running { .. } => {
            Refusal::Skipped(SkipReason::RunningProcess, error.to_string())
        }
        error @ fs::Error::RunningUnknown { .. } => {
            Refusal::Skipped(SkipReason::ProcessCheckFailed, error.to_string())
        }
        error @ fs::Error::NotRootControlled { .. } => {
            let reason = format!("{error}; delete it manually");
            Refusal::Skipped(SkipReason::UnsafeToElevate, reason)
        }
        error => Refusal::Refused(error.to_string()),
    }
}

/// The results of the refused items: each refused delete, and each bootout of a
/// refused delete whose jobs this process boots out.
fn refused(runnable: &Runnable, verdicts: &Verdicts) -> Vec<ItemResult> {
    let refusal = |delete: &Delete| match verdicts.get(delete.path.as_path()) {
        Some(Err(refusal)) => Some(refusal.clone()),
        Some(Ok(_)) | None => None,
    };

    let bootouts = runnable.holders().iter().flat_map(|delete| {
        let refused = refusal(delete);
        delete.bootouts.iter().filter_map(move |bootout| {
            let (skip, reason) = refused.clone()?.of_bootout();
            Some(withheld(bootout.absolute_argv().into(), skip, reason))
        })
    });
    let deletes = runnable.deletes().iter().filter_map(|delete| {
        let (outcome, reason) = refusal(delete)?.of_delete();
        Some(path_result(delete, outcome, reason))
    });
    bootouts.chain(deletes).collect()
}

fn path_result(delete: &Delete, outcome: Outcome, reason: String) -> ItemResult {
    ItemResult {
        subject: Subject::Path(delete.path.clone()),
        size: ItemSize::Known(delete.size),
        outcome,
        reason,
    }
}

/// The result of a command that was never tried. It is always a skip, since a done or
/// failed command is one that was tried (a failed one ran and failed, or could not be
/// started): the run that launched the elevated child deletes a user's item only after
/// its bootouts were tried.
fn withheld(argv: Argv, skip: SkipReason, reason: String) -> ItemResult {
    ItemResult {
        subject: Subject::Command {
            argv,
            output: Vec::new(),
        },
        size: ItemSize::Unknown,
        outcome: Outcome::Skipped(skip),
        reason,
    }
}

fn exit_reason(exit: Exit) -> String {
    match exit {
        Exit::Code(code) => format!("exited with code {code}"),
        Exit::Signal(signal) => format!("killed by signal {signal}"),
    }
}

/// How many items runnable entries count as: each bootout, delete, tool, and receipt.
fn item_count(runnable: &Runnable) -> usize {
    runnable.bootouts().count()
        + runnable.deletes().len()
        + runnable.tools().len()
        + runnable.receipts().len()
}

/// Every item of runnable entries, all skipped for `skip` and `reason`: what a run
/// reports for items it could not run.
fn unrun(runnable: &Runnable, skip: SkipReason, reason: &str) -> Vec<ItemResult> {
    let command = |argv: Argv| withheld(argv, skip, reason.to_owned());

    let bootouts = runnable
        .bootouts()
        .map(|b| command(b.absolute_argv().into()));
    let deletes = runnable
        .deletes()
        .iter()
        .map(|d| path_result(d, Outcome::Skipped(skip), reason.to_owned()));
    let tools = runnable.tools().iter().map(|t| command(t.command.argv()));
    let receipts = runnable
        .receipts()
        .iter()
        .map(|r| command(r.absolute_argv().into()));

    bootouts
        .chain(deletes)
        .chain(tools)
        .chain(receipts)
        .collect()
}

#[cfg(test)]
mod tests;
