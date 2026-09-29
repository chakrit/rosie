use std::io;
use std::str::Utf8Error;

use thiserror::Error;

use super::console::PromptError;
use super::session::PickKind;
use crate::run::{RootError, elevated};
use crate::{apps, config, fs, packs, plan, rules, scan};

#[derive(Debug, Error)]
pub enum Error {
    /// The command was used wrongly; rosie exits with the usage status.
    #[error("{0}")]
    Usage(String),

    #[error("cancelled")]
    Cancelled,

    #[error("there are no {} to choose from", .kind.empty())]
    NothingToPick { kind: PickKind },

    #[error("the plan is not UTF-8 text: {0}")]
    PlanNotText(#[source] Utf8Error),

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
    Elevated(#[from] elevated::Error),

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
