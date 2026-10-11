//! How each item of a run ended, and the end-of-run totals, printed to stderr
//! (`docs/spec/plan.md#stats`).

use std::fmt;

use super::ItemResult;
use crate::plan::{ItemSize, Size, Tally};

/// How one item of a run ended. A skipped item never ran. A command that is done ran;
/// one that failed ran and failed, or could not be started. A failed delete may also be
/// one the context refused before it began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Skipped(SkipReason),
    /// The item errored; the run went on with the rest.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The path is missing or its `lstat` type changed since the scan.
    Stale,
    /// A process is executing from the item.
    RunningProcess,
    /// Elevation for the `sudo` items was refused or failed.
    SudoRefused,
    /// The running programs could not be read, so a process running a program from the
    /// item, or from the delete a bootout belongs to, could not be ruled out. Unlike
    /// the other skips, it fails the run.
    ProcessCheckFailed,
    /// The context refused the delete, which fails, so its bootouts never ran and are
    /// skipped.
    DeleteRefused,
}

impl SkipReason {
    pub fn label(self) -> &'static str {
        match self {
            SkipReason::Stale => "stale",
            SkipReason::RunningProcess => "running process",
            SkipReason::SudoRefused => "sudo refused",
            SkipReason::ProcessCheckFailed => "process check failed",
            SkipReason::DeleteRefused => "delete refused",
        }
    }
}

/// The end-of-run totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunStats {
    pub done: Tally,
    pub stale: Tally,
    pub running_process: Tally,
    pub sudo_refused: Tally,
    pub process_check_failed: Tally,
    pub delete_refused: Tally,
    pub failed: Tally,
}

impl RunStats {
    pub fn record(&mut self, size: ItemSize, outcome: Outcome) {
        let tally = match outcome {
            Outcome::Done => &mut self.done,
            Outcome::Skipped(SkipReason::Stale) => &mut self.stale,
            Outcome::Skipped(SkipReason::RunningProcess) => &mut self.running_process,
            Outcome::Skipped(SkipReason::SudoRefused) => &mut self.sudo_refused,
            Outcome::Skipped(SkipReason::ProcessCheckFailed) => &mut self.process_check_failed,
            Outcome::Skipped(SkipReason::DeleteRefused) => &mut self.delete_refused,
            Outcome::Failed => &mut self.failed,
        };
        tally.add(size);
    }

    /// The bytes the done items held.
    pub fn freed(&self) -> Size {
        self.done.size
    }

    /// Whether any item failed, or could not be checked for running processes; rosie
    /// then exits non-zero (`docs/spec/cli.md#exit-status`).
    pub fn any_failed(&self) -> bool {
        !self.failed.is_empty() || !self.process_check_failed.is_empty()
    }

    /// Every skipped item, whatever the reason.
    pub fn skipped(&self) -> Tally {
        self.stale
            .plus(self.running_process)
            .plus(self.sudo_refused)
            .plus(self.process_check_failed)
            .plus(self.delete_refused)
    }

    fn skip_reasons(&self) -> [(SkipReason, Tally); 5] {
        [
            (SkipReason::Stale, self.stale),
            (SkipReason::RunningProcess, self.running_process),
            (SkipReason::SudoRefused, self.sudo_refused),
            (SkipReason::ProcessCheckFailed, self.process_check_failed),
            (SkipReason::DeleteRefused, self.delete_refused),
        ]
    }
}

impl FromIterator<ItemResult> for RunStats {
    fn from_iter<I: IntoIterator<Item = ItemResult>>(results: I) -> Self {
        let mut stats = RunStats::default();
        for result in results {
            stats.record(result.size, result.outcome);
        }
        stats
    }
}

impl fmt::Display for RunStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "freed {}; done: {}", self.freed(), self.done)?;

        let skipped = self.skipped();
        if !skipped.is_empty() {
            let reasons: Vec<String> = self
                .skip_reasons()
                .into_iter()
                .filter(|(_, tally)| !tally.is_empty())
                .map(|(reason, tally)| format!("{} {}", reason.label(), tally))
                .collect();
            writeln!(f, "skipped: {skipped} ({})", reasons.join("; "))?;
        }
        if !self.failed.is_empty() {
            writeln!(f, "failed: {}", self.failed)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tally(items: usize, bytes: u64, unknown: usize) -> Tally {
        Tally {
            items,
            size: Size::bytes(bytes),
            unknown,
        }
    }

    #[test]
    fn run_stats_split_skips_and_failures_by_reason() {
        let mut stats = RunStats::default();
        let outcomes = [
            (ItemSize::Known(Size::bytes(1_000)), Outcome::Done),
            (ItemSize::Known(Size::bytes(2_000)), Outcome::Done),
            (ItemSize::Unknown, Outcome::Done),
            (
                ItemSize::Known(Size::bytes(30)),
                Outcome::Skipped(SkipReason::Stale),
            ),
            (
                ItemSize::Known(Size::bytes(400)),
                Outcome::Skipped(SkipReason::RunningProcess),
            ),
            (
                ItemSize::Known(Size::bytes(5)),
                Outcome::Skipped(SkipReason::SudoRefused),
            ),
            (
                ItemSize::Known(Size::bytes(6)),
                Outcome::Skipped(SkipReason::SudoRefused),
            ),
            (ItemSize::Unknown, Outcome::Failed),
            (ItemSize::Known(Size::bytes(70)), Outcome::Failed),
        ];
        for (size, outcome) in outcomes {
            stats.record(size, outcome);
        }

        assert_eq!(stats.freed(), Size::bytes(3_000));
        assert_eq!(stats.done, tally(3, 3_000, 1));
        assert_eq!(stats.stale, tally(1, 30, 0));
        assert_eq!(stats.running_process, tally(1, 400, 0));
        assert_eq!(stats.sudo_refused, tally(2, 11, 0));
        assert_eq!(stats.skipped(), tally(4, 441, 0));
        assert_eq!(stats.failed, tally(2, 70, 1));
    }

    #[test]
    fn run_summary_names_every_nonzero_bucket() {
        let mut stats = RunStats::default();
        stats.record(ItemSize::Known(Size::bytes(3_400_000_000)), Outcome::Done);
        stats.record(ItemSize::Unknown, Outcome::Done);
        stats.record(
            ItemSize::Known(Size::bytes(50_000_000)),
            Outcome::Skipped(SkipReason::Stale),
        );
        stats.record(ItemSize::Known(Size::bytes(20_000)), Outcome::Failed);

        assert_eq!(
            stats.to_string(),
            "freed 3.4 GB; done: 2 items, 3.4 GB, 1 of unknown size\n\
             skipped: 1 item, 50.0 MB (stale 1 item, 50.0 MB)\n\
             failed: 1 item, 20.0 KB\n"
        );
    }
}
