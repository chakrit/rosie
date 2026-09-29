//! Executing a plan (`docs/spec/plan.md#run`, `docs/spec/safety.md#elevation`).
//!
//! A run does the user's ticked entries first, then pipes the remaining `sudo` entries
//! to one elevated child, which runs them through the same code. Within each part:
//!
//! 1. every bootout, one at a time, so no plist is deleted with its job loaded;
//! 2. the running-process table is read;
//! 3. the deletes, in parallel, each re-validated first; alongside them, the tool
//!    commands and receipts, one at a time.
//!
//! A run has up to three steps: the user's part; the elevated part; then, in the
//! user's process again, the user's deletes the elevated part only booted `system`
//! jobs out for. The elevated child deletes only through folders no one but root can
//! change (`Gate::delete_as_root`), and never deletes a user's item.
//!
//! A failed item is reported and the run goes on.

mod elevate;
pub mod elevated;
mod header;
mod item;
mod outcomes;
mod report;
mod root;

use std::io;
use std::thread;

use rayon::prelude::*;

use crate::fs::{self, Argv, Backend, Exit, Gate};
use crate::plan::{Delete, ItemSize, Outcome, Plan, RunAs, RunStats, Runnable, SkipReason};
use crate::process::{self, ProcessTable};

pub use elevate::Elevation;
pub use item::{ItemResult, Subject};
pub use report::{OutputWindow, PlainReporter, Reporter, TerminalReporter};
pub use root::{RootError, RootRefusal, refuse_root};

pub struct Runner<B: Backend> {
    gate: Gate<B>,
    /// Whom this process acts as: the user for the run itself, `Sudo` for the elevated
    /// child.
    acting: RunAs,
}

impl<B: Backend + Sync> Runner<B> {
    /// `gate` holds the roots loaded from the user's `config.toml`; a plan never
    /// carries roots.
    pub fn new(gate: Gate<B>) -> Self {
        Runner {
            gate,
            acting: RunAs::User,
        }
    }

    /// A runner for the elevated child, which acts as root on the user's behalf.
    fn as_root(gate: Gate<B>) -> Self {
        Runner {
            gate,
            acting: RunAs::Sudo,
        }
    }

    /// Runs the plan's ticked entries: the user's part here, then the `sudo` part in
    /// one elevated child, then here the user's deletes whose jobs the child booted out.
    pub fn run<R: Reporter + Send>(
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

    /// Runs the ticked entries of `plan` that this process executes, whoever it runs as.
    pub fn execute<R: Reporter + Send>(&self, plan: &Plan, reporter: &mut R) -> Vec<ItemResult> {
        self.execute_runnable(&plan.runnable_as(self.acting), reporter)
    }

    fn execute_runnable<R: Reporter + Send>(
        &self,
        runnable: &Runnable,
        reporter: &mut R,
    ) -> Vec<ItemResult> {
        let mut results: Vec<ItemResult> = runnable
            .bootouts
            .iter()
            .map(|bootout| self.run_command(bootout.absolute_argv().into(), reporter))
            .collect();

        let processes = ProcessTable::query(&self.gate);
        let commands: Vec<Argv> = runnable
            .tools
            .iter()
            .map(|tool| tool.command.argv())
            .chain(
                runnable
                    .receipts
                    .iter()
                    .map(|receipt| receipt.absolute_argv().into()),
            )
            .collect();

        let (deletes, commands) = thread::scope(|scope| {
            let commands = scope.spawn(|| {
                commands
                    .into_iter()
                    .map(|argv| self.run_command(argv, reporter))
                    .collect::<Vec<_>>()
            });
            let deletes: Vec<ItemResult> = runnable
                .deletes
                .par_iter()
                .map(|delete| self.delete(delete, &processes))
                .collect();
            let commands = commands
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            (deletes, commands)
        });

        for result in &deletes {
            reporter.item_finished(result);
        }
        results.extend(commands);
        results.extend(deletes);
        results
    }

    // deletes

    /// Re-validates one delete, then deletes it through the roots gate.
    fn delete(
        &self,
        delete: &Delete,
        processes: &Result<ProcessTable, process::Error>,
    ) -> ItemResult {
        let result = |outcome, reason| ItemResult {
            subject: Subject::Path(delete.path.clone()),
            size: ItemSize::Known(delete.size),
            outcome,
            reason,
        };

        if let Some(reason) = self.staleness(delete) {
            return result(Outcome::Skipped(SkipReason::Stale), reason);
        }
        let processes = match processes {
            Ok(table) => table,
            Err(error) => {
                let reason = format!("cannot check for running processes: {error}");
                return result(Outcome::Failed, reason);
            }
        };
        if let Some(process) = processes.executing_from(&delete.path) {
            let reason = format!("process {} runs {}", process.pid, process.comm.display());
            return result(Outcome::Skipped(SkipReason::RunningProcess), reason);
        }

        let deleted = match self.acting {
            RunAs::User => self.gate.delete(&delete.path),
            RunAs::Sudo => self.gate.delete_as_root(&delete.path),
        };
        match deleted {
            Ok(()) => result(Outcome::Done, String::new()),
            Err(error @ fs::Error::NotRootControlled { .. }) => {
                let reason = format!("{error}; delete it manually");
                result(Outcome::Skipped(SkipReason::UnsafeToElevate), reason)
            }
            Err(error) => result(Outcome::Failed, error.to_string()),
        }
    }

    /// Why the entry no longer describes what is on disk: its path is missing, or its
    /// `lstat` type changed since the scan. `None` when it still does, or when `lstat`
    /// fails otherwise and the delete is left to report it.
    fn staleness(&self, delete: &Delete) -> Option<String> {
        match self.gate.lstat(&delete.path) {
            Ok(meta) if delete.kind.matches(meta.kind) => None,
            Ok(meta) => Some(format!(
                "was a {} and is now a {}",
                delete.kind.label(),
                meta.kind.label()
            )),
            Err(fs::Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                Some("no longer exists".to_owned())
            }
            Err(_) => None,
        }
    }

    // commands

    /// Runs one command, never through a shell, showing its last 3 output lines while
    /// it runs and keeping them as its result.
    fn run_command<R: Reporter>(&self, argv: Argv, reporter: &mut R) -> ItemResult {
        reporter.command_started(&argv);

        let mut window = OutputWindow::default();
        let exit = self.gate.run_streaming(&argv, |line| {
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

fn exit_reason(exit: Exit) -> String {
    match exit {
        Exit::Code(code) => format!("exited with code {code}"),
        Exit::Signal(signal) => format!("killed by signal {signal}"),
    }
}

/// How many items runnable entries count as: each bootout, delete, tool, and receipt.
fn item_count(runnable: &Runnable) -> usize {
    runnable.bootouts.len()
        + runnable.deletes.len()
        + runnable.tools.len()
        + runnable.receipts.len()
}

/// Every item of runnable entries, all ending with `outcome` for `reason`: what a run
/// reports for items it could not run.
fn unrun(runnable: &Runnable, outcome: Outcome, reason: &str) -> Vec<ItemResult> {
    let command = |argv: Argv| {
        (
            Subject::Command {
                argv,
                output: Vec::new(),
            },
            ItemSize::Unknown,
        )
    };

    let bootouts = runnable
        .bootouts
        .iter()
        .map(|b| command(b.absolute_argv().into()));
    let deletes = runnable
        .deletes
        .iter()
        .map(|d| (Subject::Path(d.path.clone()), ItemSize::Known(d.size)));
    let tools = runnable.tools.iter().map(|t| command(t.command.argv()));
    let receipts = runnable
        .receipts
        .iter()
        .map(|r| command(r.absolute_argv().into()));

    bootouts
        .chain(deletes)
        .chain(tools)
        .chain(receipts)
        .map(|(subject, size)| ItemResult {
            subject,
            size,
            outcome,
            reason: reason.to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
