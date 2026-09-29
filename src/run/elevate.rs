//! The run's side of elevation: launching `sudo <rosie> __elevated <nonce>` with the
//! `sudo` part of the plan piped to it (`docs/spec/safety.md#elevation`), then deleting
//! the user's items the child only booted jobs out for.

use std::path::PathBuf;

use super::header::{Header, Nonce};
use super::outcomes::{self, Key, Record};
use super::{
    ItemResult, Refusal, Reporter, Runner, Subject, elevated, exit_reason, item_count, path_result,
    unrun,
};
use crate::fs::{Argv, Backend, Home};
use crate::plan::{Delete, Outcome, Plan, RunAs, Runnable, SkipReason};

/// What launching the elevated child needs, injected by `main`.
#[derive(Debug, Clone)]
pub struct Elevation {
    /// This rosie executable, as an absolute path: sudo's `PATH` may find another.
    pub exe: PathBuf,
    /// The invoking user's home, passed to the child in the stdin header.
    pub home: Home,
}

impl<B: Backend + Sync> Runner<B> {
    /// Runs the `sudo` part in one elevated child and reads back each item's outcome;
    /// the child reports each item on stderr itself. When sudo cannot start or is
    /// refused, or the child's report is unreadable or names items it was not sent,
    /// every item is skipped as `sudo refused`. Otherwise each item takes the outcome the
    /// child reported, and one it never reported is skipped as `sudo refused`. The items
    /// are what the child runs of the part: every bootout, the deletes that run as root,
    /// and the receipts.
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
    /// booted their `system` jobs out. An item is deleted only when every one of its
    /// bootouts is done, or failed (it ran and failed, or could not be started), as in
    /// any part. One whose bootout the child skipped, or never reported, is kept, since
    /// its job may still be loaded: it fails when the child's gate refused it, and is
    /// skipped with the bootout otherwise.
    pub(super) fn delete_after_elevation<R: Reporter>(
        &self,
        part: &Plan,
        elevated: &[ItemResult],
        reporter: &mut R,
    ) -> Vec<ItemResult> {
        let user_part = part.after_elevation();
        let kept: Vec<ItemResult> = user_part
            .deletes()
            .iter()
            .filter_map(|delete| {
                let (outcome, reason) = unbooted(delete, elevated)?.of_delete();
                Some(path_result(delete, outcome, reason))
            })
            .collect();
        for result in &kept {
            reporter.item_finished(result);
        }

        let booted = user_part.without(|delete| unbooted(delete, elevated).is_some());
        let mut results = kept;
        if !booted.deletes().is_empty() {
            results.extend(self.execute_runnable(&booted, reporter));
        }
        results
    }
}

const SUDO_REFUSED: SkipReason = SkipReason::SudoRefused;

/// Why a delete's job may still be loaded, as the refusal it ends with: one of its
/// bootouts was skipped in the elevated part, or was never reported. A bootout the
/// child withheld because its gate refused this delete refuses the delete here too;
/// any other skip skips the delete for the same reason. `None` only when every one is
/// done, or failed (it ran and failed, or could not be started).
fn unbooted(delete: &Delete, elevated: &[ItemResult]) -> Option<Refusal> {
    delete.bootouts.iter().find_map(|bootout| {
        let argv: Argv = bootout.absolute_argv().into();
        let result = elevated.iter().find(
            |result| matches!(&result.subject, Subject::Command { argv: ran, .. } if *ran == argv),
        );
        let Some(result) = result else {
            return Some(Refusal::Skipped(
                SUDO_REFUSED,
                format!("`{argv}` did not run"),
            ));
        };
        match result.outcome {
            Outcome::Skipped(SkipReason::DeleteRefused) => Some(Refusal::Refused(
                "the elevated run refused to delete it".to_owned(),
            )),
            Outcome::Skipped(skip) => Some(Refusal::Skipped(
                skip,
                "its job was not booted out".to_owned(),
            )),
            Outcome::Done | Outcome::Failed => None,
        }
    })
}

fn not_run<R: Reporter>(
    items: &Runnable,
    skip: SkipReason,
    reason: &str,
    reporter: &mut R,
) -> Vec<ItemResult> {
    unrun(items, skip, reason)
        .into_iter()
        .inspect(|result| reporter.item_finished(result))
        .collect()
}

/// Each item's outcome as the child reported it. An item the child never reported,
/// because it crashed or was killed first, is skipped as `sudo refused`. A report naming
/// an item it was not sent cannot be trusted, so every item is then skipped as `sudo
/// refused`.
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
