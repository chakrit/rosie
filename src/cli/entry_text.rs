//! The words a line about a plan entry uses, the same in `rosie check` and the plan
//! listing: a conflict's sides, `needs sudo`, and the hints that would change its status
//! (`docs/spec/cli.md#check`, `docs/spec/plan.md#plan-listing`).

use std::path::Path;

use crate::hint::{Flag, Hint};
use crate::plan::{Disagreement, RunAs, StatusKind};

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

/// The hints that would make an entry with status `kind` at `shown_path` clean.
pub fn status_hints(kind: StatusKind, shown_path: &Path) -> Vec<Hint> {
    match kind {
        StatusKind::Clean => Vec::new(),
        StatusKind::Keep => vec![keep_hint()],
        StatusKind::OutsideRoots => Hint::allow_in_roots(shown_path),
        StatusKind::Conflict => vec![Hint::by_hand("choose clean or keep in the plan")],
    }
}

/// What makes a `keep` item clean.
pub fn keep_hint() -> Hint {
    Hint::flag("to clean it", Flag::Aggressive)
}
