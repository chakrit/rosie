//! Plan and run statistics, printed to stderr and never written into the plan
//! (`docs/spec/plan.md#stats`).

use std::fmt;

use super::{Plan, Size, Status};

/// How much one item weighs in the totals. Tool commands are "size unknown" and are
/// excluded from the byte totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemSize {
    Known(Size),
    Unknown,
}

/// A count of items and the bytes they hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tally {
    pub items: usize,
    /// The sum of the items of known size.
    pub size: Size,
    /// How many of the items are of unknown size.
    pub unknown: usize,
}

impl Tally {
    pub fn add(&mut self, size: ItemSize) {
        self.items += 1;
        match size {
            ItemSize::Known(size) => self.size += size,
            ItemSize::Unknown => self.unknown += 1,
        }
    }

    fn plus(self, other: Tally) -> Tally {
        Tally {
            items: self.items + other.items,
            size: self.size + other.size,
            unknown: self.unknown + other.unknown,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items == 0
    }
}

/// `3 items, 1.2 GB`, adding `, 1 of unknown size` when tools are among them.
impl fmt::Display for Tally {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let noun = match self.items {
            1 => "item",
            _ => "items",
        };
        write!(f, "{} {noun}, {}", self.items, self.size)?;
        match self.unknown {
            0 => Ok(()),
            unknown => write!(f, ", {unknown} of unknown size"),
        }
    }
}

/// What scan and confirmation show about a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlanStats {
    pub ticked: Tally,
    /// Aggressive items listed without `--aggressive`.
    pub unticked: Tally,
    /// Items outside the roots.
    pub blocked: Tally,
}

impl PlanStats {
    /// Report-only entries are not items: rosie does nothing with them.
    pub fn of(plan: &Plan) -> Self {
        let deletes = plan
            .deletes()
            .iter()
            .map(|delete| (delete.status, ItemSize::Known(delete.size)));
        let tools = plan
            .tools()
            .iter()
            .map(|tool| (tool.selection.into(), ItemSize::Unknown));
        let receipts = plan
            .receipts()
            .iter()
            .map(|receipt| (receipt.selection.into(), ItemSize::Unknown));

        let mut stats = PlanStats::default();
        for (status, size) in deletes.chain(tools).chain(receipts) {
            stats.bucket(status).add(size);
        }
        stats
    }

    fn bucket(&mut self, status: Status) -> &mut Tally {
        match status {
            Status::Ticked => &mut self.ticked,
            Status::Unticked => &mut self.unticked,
            Status::Blocked => &mut self.blocked,
        }
    }
}

impl fmt::Display for PlanStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "ticked: {}", self.ticked)?;
        if !self.unticked.is_empty() {
            writeln!(f, "unticked (aggressive, not run): {}", self.unticked)?;
        }
        if !self.blocked.is_empty() {
            writeln!(f, "blocked (outside roots, not run): {}", self.blocked)?;
        }
        Ok(())
    }
}

// run

/// How one executed item ended.
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
    /// A folder above the item could be changed by someone other than root, so the
    /// elevated child left it for the user to delete by hand.
    UnsafeToElevate,
}

impl SkipReason {
    pub fn label(self) -> &'static str {
        match self {
            SkipReason::Stale => "stale",
            SkipReason::RunningProcess => "running process",
            SkipReason::SudoRefused => "sudo refused",
            SkipReason::UnsafeToElevate => "unsafe to elevate",
        }
    }
}

/// Why the walk did not enter a folder (`docs/spec/safety.md#walk-skips`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkSkip {
    Bundle,
    Mount,
    Placeholder,
    /// `EPERM` or another denial.
    Denied,
}

impl WalkSkip {
    /// `3 bundles`, `1 mount`, `2 denied`.
    fn counted(self, count: usize) -> String {
        let (one, many) = match self {
            WalkSkip::Bundle => ("bundle", "bundles"),
            WalkSkip::Mount => ("mount", "mounts"),
            WalkSkip::Placeholder => ("placeholder", "placeholders"),
            WalkSkip::Denied => ("denied", "denied"),
        };
        match count {
            1 => format!("{count} {one}"),
            _ => format!("{count} {many}"),
        }
    }
}

/// Walk-skip counts per reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WalkSkips {
    pub bundles: usize,
    pub mounts: usize,
    pub placeholders: usize,
    pub denied: usize,
}

impl WalkSkips {
    pub fn record(&mut self, skip: WalkSkip) {
        let count = match skip {
            WalkSkip::Bundle => &mut self.bundles,
            WalkSkip::Mount => &mut self.mounts,
            WalkSkip::Placeholder => &mut self.placeholders,
            WalkSkip::Denied => &mut self.denied,
        };
        *count += 1;
    }

    pub fn total(&self) -> usize {
        self.bundles + self.mounts + self.placeholders + self.denied
    }

    fn counts(&self) -> [(WalkSkip, usize); 4] {
        [
            (WalkSkip::Bundle, self.bundles),
            (WalkSkip::Mount, self.mounts),
            (WalkSkip::Placeholder, self.placeholders),
            (WalkSkip::Denied, self.denied),
        ]
    }
}

/// `walk skipped 3 bundles, 2 denied`; empty when nothing was skipped.
impl fmt::Display for WalkSkips {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.total() == 0 {
            return Ok(());
        }

        let parts: Vec<String> = self
            .counts()
            .into_iter()
            .filter(|(_, count)| *count > 0)
            .map(|(skip, count)| skip.counted(count))
            .collect();
        write!(f, "walk skipped {}", parts.join(", "))
    }
}

/// The end-of-run totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunStats {
    pub done: Tally,
    pub stale: Tally,
    pub running_process: Tally,
    pub sudo_refused: Tally,
    pub unsafe_to_elevate: Tally,
    pub failed: Tally,
    pub walk_skips: WalkSkips,
}

impl RunStats {
    pub fn record(&mut self, size: ItemSize, outcome: Outcome) {
        let tally = match outcome {
            Outcome::Done => &mut self.done,
            Outcome::Skipped(SkipReason::Stale) => &mut self.stale,
            Outcome::Skipped(SkipReason::RunningProcess) => &mut self.running_process,
            Outcome::Skipped(SkipReason::SudoRefused) => &mut self.sudo_refused,
            Outcome::Skipped(SkipReason::UnsafeToElevate) => &mut self.unsafe_to_elevate,
            Outcome::Failed => &mut self.failed,
        };
        tally.add(size);
    }

    /// The bytes the done items held.
    pub fn freed(&self) -> Size {
        self.done.size
    }

    /// Whether any item failed; rosie then exits non-zero (`docs/spec/cli.md#exit-status`).
    pub fn any_failed(&self) -> bool {
        !self.failed.is_empty()
    }

    /// Every skipped item, whatever the reason.
    pub fn skipped(&self) -> Tally {
        self.stale
            .plus(self.running_process)
            .plus(self.sudo_refused)
            .plus(self.unsafe_to_elevate)
    }

    fn skip_reasons(&self) -> [(SkipReason, Tally); 4] {
        [
            (SkipReason::Stale, self.stale),
            (SkipReason::RunningProcess, self.running_process),
            (SkipReason::SudoRefused, self.sudo_refused),
            (SkipReason::UnsafeToElevate, self.unsafe_to_elevate),
        ]
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
        if self.walk_skips.total() > 0 {
            writeln!(f, "{}", self.walk_skips)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::plan::{
        AggressiveItems, ItemKind, LaunchDomain, PathMatch, PlanBuilder, Reach, Report, RunAs,
        ToolCmds, Twin,
    };

    fn found(path: &str, bytes: u64, reach: Reach, twin: Twin) -> PathMatch {
        PathMatch {
            path: PathBuf::from(path),
            kind: ItemKind::Folder,
            size: Size::bytes(bytes),
            run_as: RunAs::User,
            reach,
            rule: "rosie/r".to_owned(),
            twin,
        }
    }

    fn tally(items: usize, bytes: u64, unknown: usize) -> Tally {
        Tally {
            items,
            size: Size::bytes(bytes),
            unknown,
        }
    }

    #[test]
    fn plan_stats_total_ticked_unticked_and_blocked_items() {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);
        let matches = [
            found("/w/a", 1_000, Reach::InRoots, Twin::Normal),
            found("/w/b", 2_500, Reach::InRoots, Twin::Normal),
            found("/w/c", 40_000, Reach::InRoots, Twin::Aggressive),
            found("/x/d", 7, Reach::OutsideRoots, Twin::Normal),
        ];
        for found in matches {
            builder.add_path(found).expect("valid match");
        }
        let docker = ToolCmds::Twins {
            cmd: "docker system prune --force",
            cmd_aggressive: "docker system prune --all --force",
        };
        builder
            .add_tool("rosie/docker", docker)
            .expect("valid command");
        builder
            .add_receipt("com.x.pkg", Twin::Normal)
            .expect("valid");
        builder
            .add_launch_job(
                found("/w/e.plist", 300, Reach::InRoots, Twin::Normal),
                LaunchDomain::System,
            )
            .expect("valid plist");
        builder.add_report(Report::new("login item X".to_owned(), vec![]).expect("valid"));

        let stats = PlanStats::of(&builder.build());

        assert_eq!(
            stats,
            PlanStats {
                ticked: tally(5, 3_800, 2),
                unticked: tally(2, 40_000, 1),
                blocked: tally(1, 7, 0),
            }
        );
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
            (
                ItemSize::Known(Size::bytes(8)),
                Outcome::Skipped(SkipReason::UnsafeToElevate),
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
        assert_eq!(stats.unsafe_to_elevate, tally(1, 8, 0));
        assert_eq!(stats.skipped(), tally(5, 449, 0));
        assert_eq!(stats.failed, tally(2, 70, 1));
    }

    #[test]
    fn walk_skips_count_per_reason() {
        let mut skips = WalkSkips::default();
        let seen = [
            WalkSkip::Bundle,
            WalkSkip::Denied,
            WalkSkip::Bundle,
            WalkSkip::Placeholder,
        ];
        for skip in seen {
            skips.record(skip);
        }

        assert_eq!(
            skips,
            WalkSkips {
                bundles: 2,
                mounts: 0,
                placeholders: 1,
                denied: 1,
            }
        );
        assert_eq!(
            skips.to_string(),
            "walk skipped 2 bundles, 1 placeholder, 1 denied"
        );
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
        stats.walk_skips.record(WalkSkip::Mount);

        assert_eq!(
            stats.to_string(),
            "freed 3.4 GB; done: 2 items, 3.4 GB, 1 of unknown size\n\
             skipped: 1 item, 50.0 MB (stale 1 item, 50.0 MB)\n\
             failed: 1 item, 20.0 KB\n\
             walk skipped 1 mount\n"
        );
    }
}
