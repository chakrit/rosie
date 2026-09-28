use std::fmt;
use std::path::PathBuf;

use crate::fs::Argv;
use crate::plan::{ItemSize, Outcome};

/// How one item of a run ended, and what it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemResult {
    pub subject: Subject,
    pub size: ItemSize,
    pub outcome: Outcome,
    /// Why the item was skipped or failed; empty when it was done.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// A file or folder deleted through the roots gate.
    Path(PathBuf),
    /// A command, with the last lines of its output kept as its result.
    Command { argv: Argv, output: Vec<String> },
}

/// `deleted /path (1.2 GB)`, `skipped /path (stale): no longer exists`, or `ran
/// docker system prune --force` followed by its kept output lines.
impl fmt::Display for ItemResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = match (&self.subject, self.outcome) {
            (Subject::Path(_), Outcome::Done) => "deleted".to_owned(),
            (Subject::Command { .. }, Outcome::Done) => "ran".to_owned(),
            (_, Outcome::Skipped(reason)) => format!("skipped ({})", reason.label()),
            (_, Outcome::Failed) => "failed".to_owned(),
        };
        let what = match &self.subject {
            Subject::Path(path) => path.display().to_string(),
            Subject::Command { argv, .. } => argv.to_string(),
        };

        write!(f, "{verb} {what}")?;
        if let ItemSize::Known(size) = self.size {
            write!(f, " ({size})")?;
        }
        if !self.reason.is_empty() {
            write!(f, ": {}", self.reason)?;
        }
        if let Subject::Command { output, .. } = &self.subject {
            for line in output {
                write!(f, "\n  | {line}")?;
            }
        }
        Ok(())
    }
}
