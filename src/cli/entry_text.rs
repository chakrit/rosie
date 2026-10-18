//! The words a line about a plan entry uses, the same in `rosie check` and the plan
//! listing: a conflict's sides, `needs sudo`, and the hints that would change its status
//! (`docs/spec/cli.md#check`, `docs/spec/plan.md#plan-listing`).

use std::path::Path;

use crate::hint::{Flag, Hint};
use crate::plan::{Disagreement, RunAs, SkipReason, Status};

/// `rosie/a (normal) vs alice/b (aggressive)`.
pub fn sides(disagreement: &Disagreement) -> String {
    format!(
        "{} (normal) vs {} (aggressive)",
        disagreement.normal().join(", "),
        disagreement.aggressive().join(", ")
    )
}

/// `needs sudo` for an entry not owned by the user.
pub fn sudo_note(run_as: RunAs) -> Option<String> {
    (run_as == RunAs::Sudo).then(|| "needs sudo".to_owned())
}

/// The hints that would make an entry with `status` at `shown_path` clean: none for a
/// conflict, which only an answer settles.
pub fn status_hints(status: &Status, shown_path: &Path) -> Vec<Hint> {
    match status {
        Status::Skip(SkipReason::Aggressive) => vec![aggressive_hint()],
        Status::Skip(SkipReason::OutsideRoots) => Hint::allow_in_roots(shown_path),
        Status::Clean | Status::Skip(SkipReason::Chosen) | Status::Conflict(_) => Vec::new(),
    }
}

/// What makes an aggressive item clean.
pub fn aggressive_hint() -> Hint {
    Hint::flag("to clean it", Flag::Aggressive)
}
