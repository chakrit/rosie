use std::io;
use std::path::PathBuf;
use std::str::Utf8Error;

use thiserror::Error;

use super::args::Place;
use super::console::PromptError;
use super::root::RootError;
use super::session::PickKind;
use crate::{apps, config, fs, packs, plan, rules, run, scan};

#[derive(Debug, Error)]
pub enum Error {
    /// The command was used wrongly; rosie exits with the usage status.
    #[error("{0}")]
    Usage(String),

    /// A folder given in place of a command or mode name that does not exist; rosie
    /// exits with the usage status (`docs/spec/cli.md#modes`).
    #[error("no command or folder named {}", .word.display())]
    UnknownWord { word: PathBuf, place: Place },

    #[error("cancelled")]
    Cancelled,

    #[error("there are no {} to choose from", .kind.empty())]
    NothingToPick { kind: PickKind },

    #[error("the plan is not UTF-8 text: {0}")]
    PlanNotText(#[source] Utf8Error),

    /// `rosie run` with no terminal to ask on and no `--conflicts`; `lines` is where each
    /// conflict's `status` sits in `plan`, the file as typed or `-`
    /// (`docs/spec/plan.md#rule-conflicts`).
    #[error("{plan}: unresolved conflicts")]
    UnansweredConflicts { plan: String, lines: Vec<usize> },

    #[error(transparent)]
    Root(#[from] RootError),

    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error(transparent)]
    Config(#[from] config::Error),

    #[error(transparent)]
    Packs(#[from] packs::Error),

    #[error(transparent)]
    Rules(#[from] rules::Error),

    #[error(transparent)]
    Scan(#[from] scan::Error),

    #[error(transparent)]
    Apps(#[from] apps::Error),

    #[error(transparent)]
    Plan(#[from] plan::Error),

    #[error(transparent)]
    Run(#[from] run::Error),

    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<PromptError> for Error {
    fn from(error: PromptError) -> Self {
        match error {
            PromptError::Cancelled => Error::Cancelled,
            PromptError::Io(error) => Error::Io(error),
        }
    }
}
