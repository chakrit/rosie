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

/// How one item of a run ended. A skipped item never ran. A command that is done ran;
/// one that failed ran and failed, or could not be started. A failed delete may also be
/// one the gate refused before it began.
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
    /// The item, or a folder above or inside it, could be changed by someone other than
    /// root, so the elevated child left it for the user to delete by hand (see
    /// [`crate::fs::Error::NotRootControlled`]).
    UnsafeToElevate,
    /// The running-process table could not be read, so a process executing from the
    /// item, or from the delete a bootout belongs to, could not be ruled out. Unlike
    /// the other skips, it fails the run.
    ProcessCheckFailed,
    /// The gate refused the delete, which fails, so its bootouts never ran and are
    /// skipped.
    DeleteRefused,
}

impl SkipReason {
    pub fn label(self) -> &'static str {
        match self {
            SkipReason::Stale => "stale",
            SkipReason::RunningProcess => "running process",
            SkipReason::SudoRefused => "sudo refused",
            SkipReason::UnsafeToElevate => "unsafe to elevate",
            SkipReason::ProcessCheckFailed => "process check failed",
            SkipReason::DeleteRefused => "delete refused",
        }
    }
}

/// Why the walk did not enter a folder (`docs/spec/safety.md#walk-skips`). A closed
/// skip is one its `[walk]` settings would open, and it carries every setting that
/// takes; a sealed one stays closed whatever the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkSkip {
    /// An entry left closed until every setting in `Needs` is on: a folder no rule was
    /// seen to match, since opening it could reveal targets inside, or a fixed path or
    /// leftover location that lies inside a mount the walk may not enter.
    Closed(Needs),
    /// A mount root that is never entered and never a target, whatever the settings:
    /// one a rule matches, since the gate never deletes a mount point, or one inside an
    /// item being sized, since sizing never leaves the item's volume.
    SealedMount,
    /// A placeholder that is never opened and never a target, whatever the settings:
    /// one a rule matches, since a match is final, or one inside an item being sized.
    SealedPlaceholder,
    /// `EPERM` or another denial.
    Denied,
}

impl WalkSkip {
    /// Why the path was skipped, briefly, as the line naming it shows it
    /// (`docs/spec/plan.md`): each closure of a closed skip, outermost first.
    pub fn reason(self) -> String {
        match self {
            WalkSkip::Closed(needs) => {
                let reasons: Vec<&str> = needs.settings().map(WalkSetting::reason).collect();
                reasons.join("; ")
            }
            WalkSkip::SealedMount => "mount point, never entered or a target".to_owned(),
            WalkSkip::SealedPlaceholder => "cloud placeholder, never opened or a target".to_owned(),
            WalkSkip::Denied => "permission denied".to_owned(),
        }
    }

    /// The count this skip adds to: a closed skip counts under its outermost closure.
    fn bucket(self) -> Bucket {
        match self {
            WalkSkip::Closed(needs) => match needs.outermost() {
                WalkSetting::Placeholders => Bucket::Placeholders,
                WalkSetting::Mounts => Bucket::Mounts,
                WalkSetting::Bundles => Bucket::Bundles,
            },
            WalkSkip::SealedMount => Bucket::Mounts,
            WalkSkip::SealedPlaceholder => Bucket::Placeholders,
            WalkSkip::Denied => Bucket::Denied,
        }
    }
}

/// A `[walk]` setting that opens a kind of closed folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkSetting {
    /// `enter_placeholders`: a dataless cloud placeholder.
    Placeholders,
    /// `enter_mounts`: a mount root on another volume.
    Mounts,
    /// `enter_bundles`: a bundle folder.
    Bundles,
}

impl WalkSetting {
    fn reason(self) -> &'static str {
        match self {
            WalkSetting::Placeholders => "cloud placeholder, not downloaded",
            WalkSetting::Mounts => "another volume",
            WalkSetting::Bundles => "bundle",
        }
    }
}

/// The `[walk]` settings a closed folder needs, all together, before the walk enters
/// it. Never empty: it starts from one setting and only grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Needs {
    placeholders: bool,
    mounts: bool,
    bundles: bool,
}

impl Needs {
    pub const PLACEHOLDERS: Needs = Needs::of(WalkSetting::Placeholders);
    pub const MOUNTS: Needs = Needs::of(WalkSetting::Mounts);
    pub const BUNDLES: Needs = Needs::of(WalkSetting::Bundles);

    pub const fn of(setting: WalkSetting) -> Needs {
        Needs {
            placeholders: matches!(setting, WalkSetting::Placeholders),
            mounts: matches!(setting, WalkSetting::Mounts),
            bundles: matches!(setting, WalkSetting::Bundles),
        }
    }

    /// These settings, and `more` when there is one.
    pub fn and(self, more: Option<WalkSetting>) -> Needs {
        let more = more.map_or(self, Needs::of);
        Needs {
            placeholders: self.placeholders || more.placeholders,
            mounts: self.mounts || more.mounts,
            bundles: self.bundles || more.bundles,
        }
    }

    pub fn contains(self, setting: WalkSetting) -> bool {
        match setting {
            WalkSetting::Placeholders => self.placeholders,
            WalkSetting::Mounts => self.mounts,
            WalkSetting::Bundles => self.bundles,
        }
    }

    /// The settings, outermost first: placeholders, then mounts, then bundles.
    pub fn settings(self) -> impl Iterator<Item = WalkSetting> {
        [
            WalkSetting::Placeholders,
            WalkSetting::Mounts,
            WalkSetting::Bundles,
        ]
        .into_iter()
        .filter(move |setting| self.contains(*setting))
    }

    /// The first of [`Needs::settings`]; there is always one.
    pub fn outermost(self) -> WalkSetting {
        match (self.placeholders, self.mounts) {
            (true, _) => WalkSetting::Placeholders,
            (false, true) => WalkSetting::Mounts,
            (false, false) => WalkSetting::Bundles,
        }
    }
}

/// One count of [`WalkSkips`].
#[derive(Debug, Clone, Copy)]
enum Bucket {
    Bundles,
    Mounts,
    Placeholders,
    Denied,
}

impl Bucket {
    /// `3 bundles`, `1 mount`, `2 denied`.
    fn counted(self, count: usize) -> String {
        let (one, many) = match self {
            Bucket::Bundles => ("bundle", "bundles"),
            Bucket::Mounts => ("mount", "mounts"),
            Bucket::Placeholders => ("placeholder", "placeholders"),
            Bucket::Denied => ("denied", "denied"),
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
        let count = match skip.bucket() {
            Bucket::Bundles => &mut self.bundles,
            Bucket::Mounts => &mut self.mounts,
            Bucket::Placeholders => &mut self.placeholders,
            Bucket::Denied => &mut self.denied,
        };
        *count += 1;
    }

    pub fn total(&self) -> usize {
        self.bundles + self.mounts + self.placeholders + self.denied
    }

    fn counts(&self) -> [(Bucket, usize); 4] {
        [
            (Bucket::Bundles, self.bundles),
            (Bucket::Mounts, self.mounts),
            (Bucket::Placeholders, self.placeholders),
            (Bucket::Denied, self.denied),
        ]
    }
}

impl FromIterator<WalkSkip> for WalkSkips {
    fn from_iter<I: IntoIterator<Item = WalkSkip>>(skips: I) -> Self {
        let mut counts = WalkSkips::default();
        for skip in skips {
            counts.record(skip);
        }
        counts
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
            .map(|(bucket, count)| bucket.counted(count))
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
    pub process_check_failed: Tally,
    pub delete_refused: Tally,
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
            .plus(self.unsafe_to_elevate)
            .plus(self.process_check_failed)
            .plus(self.delete_refused)
    }

    fn skip_reasons(&self) -> [(SkipReason, Tally); 6] {
        [
            (SkipReason::Stale, self.stale),
            (SkipReason::RunningProcess, self.running_process),
            (SkipReason::SudoRefused, self.sudo_refused),
            (SkipReason::UnsafeToElevate, self.unsafe_to_elevate),
            (SkipReason::ProcessCheckFailed, self.process_check_failed),
            (SkipReason::DeleteRefused, self.delete_refused),
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
    use super::*;
    use crate::plan::{
        AggressiveItems, ItemKind, LaunchDomain, PathMatch, PlanBuilder, Reach, Report, RunAs,
        ToolCmds, Twin,
    };

    fn found(path: &str, bytes: u64, reach: Reach, twin: Twin) -> PathMatch {
        PathMatch {
            path: crate::plan::idle(path),
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
            cmd: &crate::plan::argv("docker system prune --force"),
            cmd_aggressive: &crate::plan::argv("docker system prune --all --force"),
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
            WalkSkip::Closed(Needs::BUNDLES),
            WalkSkip::Denied,
            WalkSkip::Closed(Needs::BUNDLES),
            WalkSkip::Closed(Needs::PLACEHOLDERS),
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
        stats.walk_skips.record(WalkSkip::Closed(Needs::MOUNTS));

        assert_eq!(
            stats.to_string(),
            "freed 3.4 GB; done: 2 items, 3.4 GB, 1 of unknown size\n\
             skipped: 1 item, 50.0 MB (stale 1 item, 50.0 MB)\n\
             failed: 1 item, 20.0 KB\n\
             walk skipped 1 mount\n"
        );
    }
}
