//! The run's side of elevation: launching `sudo <rosie> __elevated <nonce>` with the
//! `sudo` part of the plan piped to it (`docs/spec/safety.md#elevation`), then deleting
//! the user's items the child only booted jobs out for.

use std::path::PathBuf;

use super::header::{Header, Nonce};
use super::outcomes::{self, Key, Record};
use super::{ItemResult, Reporter, Runner, Subject, elevated, exit_reason, item_count, unrun};
use crate::fs::{Argv, Backend};
use crate::plan::{Delete, ItemSize, Outcome, Plan, RunAs, Runnable, SkipReason};

/// What launching the elevated child needs, injected by `main`.
#[derive(Debug, Clone)]
pub struct Elevation {
    /// This rosie executable, as an absolute path: sudo's `PATH` may find another.
    pub exe: PathBuf,
    /// The invoking user's home, passed to the child in the stdin header.
    pub home: PathBuf,
}

impl<B: Backend + Sync> Runner<B> {
    /// Runs the `sudo` part in one elevated child and reads back each item's outcome;
    /// the child reports each item on stderr itself. When sudo is refused or fails,
    /// every item is skipped as `sudo refused`. The items are what the child runs of the
    /// part: every bootout, the deletes that run as root, and the receipts.
    pub(super) fn elevate<R: Reporter>(
        &self,
        part: &Plan,
        elevation: &Elevation,
        reporter: &mut R,
    ) -> Vec<ItemResult> {
        let items = part.runnable_as(RunAs::Sudo);
        let nonce = Nonce::generate();
        let header = Header {
            nonce: nonce.clone(),
            home: elevation.home.clone(),
        };
        let stdin = part
            .to_toml()
            .map_err(|error| error.to_string())
            .and_then(|body| header.encode(&body).map_err(|error| error.to_string()));
        let stdin = match stdin {
            Ok(stdin) => stdin,
            Err(reason) => return not_run(&items, SUDO_REFUSED, &reason, reporter),
        };
        let argv = Argv::new(&elevation.exe)
            .arg(elevated::SUBCOMMAND)
            .arg(nonce.as_str());

        reporter.elevating(item_count(&items));
        let output = match self.gate.sudo(&argv, &stdin) {
            Ok(output) => output,
            Err(error) => return not_run(&items, SUDO_REFUSED, &error.to_string(), reporter),
        };
        let sudo_exit = exit_reason(output.exit);
        match outcomes::decode(&output.stdout) {
            Ok(records) => merge(&items, records, &sudo_exit, reporter),
            Err(error) => {
                let reason = format!("sudo {sudo_exit}; {error}");
                not_run(&items, SUDO_REFUSED, &reason, reporter)
            }
        }
    }

    /// Deletes, in this process, the user's items of the `sudo` part: the child only
    /// booted their `system` jobs out. An item whose bootout never ran is skipped with
    /// it, since its job may still be loaded; one whose bootout ran and failed is
    /// deleted, as in any part.
    pub(super) fn delete_after_elevation<R: Reporter + Send>(
        &self,
        part: &Plan,
        elevated: &[ItemResult],
        reporter: &mut R,
    ) -> Vec<ItemResult> {
        let mut booted = Vec::new();
        let mut results = Vec::new();
        for delete in part.runnable_as(RunAs::User).deletes {
            match unbooted(delete, elevated) {
                None => booted.push(delete),
                Some(reason) => {
                    let result = ItemResult {
                        subject: Subject::Path(delete.path.clone()),
                        size: ItemSize::Known(delete.size),
                        outcome: SUDO_REFUSED,
                        reason,
                    };
                    reporter.item_finished(&result);
                    results.push(result);
                }
            }
        }

        if !booted.is_empty() {
            let runnable = Runnable {
                bootouts: Vec::new(),
                deletes: booted,
                tools: Vec::new(),
                receipts: Vec::new(),
            };
            results.extend(self.execute_runnable(&runnable, reporter));
        }
        results
    }
}

const SUDO_REFUSED: Outcome = Outcome::Skipped(SkipReason::SudoRefused);

/// Why a delete's job may still be loaded: one of its bootouts did not run in the
/// elevated part. `None` when every one ran, done or failed.
fn unbooted(delete: &Delete, elevated: &[ItemResult]) -> Option<String> {
    delete.bootouts.iter().find_map(|bootout| {
        let argv = bootout.absolute_argv();
        let result = elevated.iter().find(
            |result| matches!(&result.subject, Subject::Command { argv: ran, .. } if *ran == argv),
        );
        match result {
            Some(result) if matches!(result.outcome, Outcome::Skipped(_)) => {
                Some(format!("its job was not booted out: {}", result.reason))
            }
            Some(_) => None,
            None => Some(format!("`{argv}` did not run")),
        }
    })
}

fn not_run<R: Reporter>(
    items: &Runnable,
    outcome: Outcome,
    reason: &str,
    reporter: &mut R,
) -> Vec<ItemResult> {
    unrun(items, outcome, reason)
        .into_iter()
        .inspect(|result| reporter.item_finished(result))
        .collect()
}

/// Each item's outcome as the child reported it. An item the child never reported,
/// because it crashed or was killed first, is skipped as `sudo refused`.
fn merge<R: Reporter>(
    items: &Runnable,
    records: Vec<Record>,
    sudo_exit: &str,
    reporter: &mut R,
) -> Vec<ItemResult> {
    let reason = format!("sudo {sudo_exit}; the elevated run ended before reporting it");
    let unreported = unrun(items, SUDO_REFUSED, &reason);

    let mut unclaimed = records;
    let reported: Vec<Option<Outcome>> = unreported
        .iter()
        .map(|item| {
            let key = Key::of(&item.subject);
            let index = unclaimed.iter().position(|record| record.key == key);
            index.map(|index| unclaimed.swap_remove(index).outcome)
        })
        .collect();
    if !unclaimed.is_empty() {
        let reason = format!(
            "sudo {sudo_exit}; the elevated run reported {} items it was not sent",
            unclaimed.len()
        );
        return not_run(items, SUDO_REFUSED, &reason, reporter);
    }

    unreported
        .into_iter()
        .zip(reported)
        .map(|(item, reported)| match reported {
            Some(outcome) => ItemResult {
                outcome,
                reason: String::new(),
                ..item
            },
            None => {
                reporter.item_finished(&item);
                item
            }
        })
        .collect()
}
