//! What a scan says about one path: the item at or above it that decides it, or what
//! lies inside it (`docs/spec/cli.md#check`).

use std::path::Path;

use super::{Problem, Refused, Scan, Skipped};
use crate::plan::{Delete, Disagreement, PlanStats, SkipReason, Status};
use crate::tier::Tier;

/// What the plan would do with a path, judged from a scan of it.
#[derive(Debug)]
pub enum PathVerdict<'s> {
    /// A plan entry at the path or holding it.
    Entry {
        delete: &'s Delete,
        planned: Planned<'s>,
        place: Place,
    },
    /// An item a process runs from, at the path or holding it.
    InUse {
        refused: &'s Refused,
        place: Place,
    },
    /// A walk skip at the path or above it.
    Skipped(&'s Skipped),
    /// A failed read at the path or above it, so the walk never judged the path.
    Unread(&'s Problem),
    /// Plan entries strictly inside the path.
    Holds(PlanStats),
    /// No plan entry inside the path, but items refused, skipped, or unread inside it;
    /// `skipped` counts the walk skips and the failed reads.
    HoldsUnplanned {
        in_use: usize,
        skipped: usize,
    },
    NoRule,
}

/// A plan entry's status, with the tier its rules agree on where it shows.
#[derive(Debug)]
pub enum Planned<'s> {
    Clean(Tier),
    Skip(SkipReason),
    Conflict(&'s Disagreement),
}

/// Where the item a verdict names sits against the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    At,
    Inside,
}

impl Scan {
    /// The verdict for `path`, a real path, most telling first: an item a process runs
    /// from at or above it, a plan entry at or above it, a walk skip at or above it, a
    /// failed read at or above it, the plan entries inside it, the refused, skipped, and
    /// unread items inside it, and otherwise no rule.
    pub(crate) fn verdict(&self, path: &Path) -> PathVerdict<'_> {
        let place = |at: &Path| match at == path {
            true => Place::At,
            false => Place::Inside,
        };
        let holds_path = |at: &Path| path.starts_with(at);
        let inside_path = |at: &Path| at != path && at.starts_with(path);

        let refused = self
            .refused
            .iter()
            .find(|refused| holds_path(&refused.path));
        if let Some(refused) = refused {
            let place = place(&refused.path);
            return PathVerdict::InUse { refused, place };
        }
        let deletes = self.plan.deletes();
        if let Some(delete) = deletes.iter().find(|delete| holds_path(&delete.path)) {
            let planned = self.planned(delete);
            let place = place(&delete.path);
            return PathVerdict::Entry {
                delete,
                planned,
                place,
            };
        }
        if let Some(skipped) = self.skipped.iter().find(|skip| holds_path(&skip.met_at)) {
            return PathVerdict::Skipped(skipped);
        }
        let unread = self
            .problems
            .iter()
            .find(|problem| problem.met_at().is_some_and(holds_path));
        if let Some(problem) = unread {
            return PathVerdict::Unread(problem);
        }

        let inside: Vec<&Delete> = deletes
            .iter()
            .filter(|delete| inside_path(&delete.path))
            .collect();
        if !inside.is_empty() {
            return PathVerdict::Holds(PlanStats::of_deletes(inside));
        }
        let in_use = self
            .refused
            .iter()
            .filter(|refused| inside_path(&refused.path))
            .count();
        let walk_skips = self
            .skipped
            .iter()
            .filter(|skip| inside_path(&skip.met_at))
            .count();
        let unread_inside = self
            .problems
            .iter()
            .filter(|problem| problem.met_at().is_some_and(inside_path))
            .count();
        let skipped = walk_skips + unread_inside;
        match (in_use, skipped) {
            (0, 0) => PathVerdict::NoRule,
            _ => PathVerdict::HoldsUnplanned { in_use, skipped },
        }
    }

    /// `delete`'s status, with the tier its rules agree on when it is `clean`.
    fn planned<'s>(&self, delete: &'s Delete) -> Planned<'s> {
        match &delete.status {
            Status::Clean => {
                let tier = self.tiers.of(&delete.path);
                Planned::Clean(tier.expect("the plan builder records the tier of each clean entry"))
            }
            Status::Skip(reason) => Planned::Skip(*reason),
            Status::Conflict(disagreement) => Planned::Conflict(disagreement),
        }
    }
}
