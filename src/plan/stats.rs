//! Plan statistics, printed to stderr and never written into the plan
//! (`docs/spec/plan.md#stats`).

use std::fmt;

use super::{Delete, Plan, Selection, Size, SkipReason, Status, StatusKind};

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

    pub(crate) fn plus(self, other: Tally) -> Tally {
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
    pub clean: Tally,
    /// Aggressive items scanned without `--aggressive`.
    pub aggressive: Tally,
    /// Items outside the roots.
    pub outside_roots: Tally,
    /// Items the user answered `skip`.
    pub chosen: Tally,
    /// Items whose rules disagree on their tier, awaiting an answer.
    pub conflicts: Tally,
}

/// One tally of a [`PlanStats`] and the words it is shown with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counted {
    pub kind: StatusKind,
    /// `clean`, `skip (aggressive)`, `skip (outside the roots)`, `skip`, or `conflict`.
    pub label: &'static str,
    pub tally: Tally,
}

impl PlanStats {
    /// Report-only entries are not items: rosie does nothing with them.
    pub fn of(plan: &Plan) -> Self {
        let commands = plan
            .tools()
            .iter()
            .map(|tool| tool.selection)
            .chain(plan.receipts().iter().map(|receipt| receipt.selection));

        let mut stats = PlanStats::of_deletes(plan.deletes());
        for selection in commands {
            let tally = match selection {
                Selection::Clean => &mut stats.clean,
                Selection::Skip => &mut stats.aggressive,
            };
            tally.add(ItemSize::Unknown);
        }
        stats
    }

    /// The tallies of `deletes` alone, such as the plan's entries inside one folder.
    pub fn of_deletes<'d>(deletes: impl IntoIterator<Item = &'d Delete>) -> Self {
        let mut stats = PlanStats::default();
        for delete in deletes {
            stats
                .bucket(&delete.status)
                .add(ItemSize::Known(delete.size));
        }
        stats
    }

    /// Every tally with its words, in the order the stats show them.
    pub fn counted(&self) -> [Counted; 5] {
        let counted = |kind, label, tally| Counted { kind, label, tally };
        [
            counted(StatusKind::Clean, "clean", self.clean),
            counted(StatusKind::Skip, "skip (aggressive)", self.aggressive),
            counted(
                StatusKind::Skip,
                "skip (outside the roots)",
                self.outside_roots,
            ),
            counted(StatusKind::Skip, "skip", self.chosen),
            counted(StatusKind::Conflict, "conflict", self.conflicts),
        ]
    }

    fn bucket(&mut self, status: &Status) -> &mut Tally {
        match status {
            Status::Clean => &mut self.clean,
            Status::Skip(SkipReason::Aggressive) => &mut self.aggressive,
            Status::Skip(SkipReason::OutsideRoots) => &mut self.outside_roots,
            Status::Skip(SkipReason::Chosen) => &mut self.chosen,
            Status::Conflict(_) => &mut self.conflicts,
        }
    }
}

/// `clean` always, then each other tally that counts anything.
impl fmt::Display for PlanStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [clean, rest @ ..] = self.counted();
        writeln!(f, "{}: {}", clean.label, clean.tally)?;
        for counted in rest.iter().filter(|counted| !counted.tally.is_empty()) {
            writeln!(f, "{}: {}", counted.label, counted.tally)?;
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
    };
    use crate::tier::{Tier, Twin};

    fn found(path: &str, bytes: u64, reach: Reach, tier: Tier) -> PathMatch {
        PathMatch {
            path: PathBuf::from(path),
            kind: ItemKind::Folder,
            size: Size::bytes(bytes),
            run_as: RunAs::User,
            reach,
            rule: "rosie/r".to_owned(),
            tier,
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
    fn plan_stats_total_clean_items_and_skips_by_reason() {
        let mut builder = PlanBuilder::new(AggressiveItems::Skip);
        let matches = [
            found("/w/a", 1_000, Reach::InRoots, Tier::Normal),
            found("/w/b", 2_500, Reach::InRoots, Tier::Normal),
            found("/w/c", 40_000, Reach::InRoots, Tier::Aggressive),
            found("/x/d", 7, Reach::OutsideRoots, Tier::Normal),
        ];
        for found in matches {
            builder.add_path(found).expect("valid match");
        }
        let prune = crate::plan::tool_command("docker system prune --force");
        let prune_all = crate::plan::tool_command("docker system prune --all --force");
        let docker = Twin::Both {
            normal: prune,
            aggressive: prune_all,
        };
        builder.add_tool("rosie/docker", &docker);
        builder
            .add_receipt("com.x.pkg", Tier::Normal)
            .expect("valid");
        builder
            .add_launch_job(
                found("/w/e.plist", 300, Reach::InRoots, Tier::Normal),
                LaunchDomain::System,
            )
            .expect("valid plist");
        builder.add_report(Report::new("login item X".to_owned(), vec![]).expect("valid"));

        let stats = PlanStats::of(&builder.build().plan);

        assert_eq!(
            stats,
            PlanStats {
                clean: tally(5, 3_800, 2),
                aggressive: tally(2, 40_000, 1),
                outside_roots: tally(1, 7, 0),
                chosen: Tally::default(),
                conflicts: Tally::default(),
            }
        );
    }
}
