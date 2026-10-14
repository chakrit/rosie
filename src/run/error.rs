use std::io;
use std::path::PathBuf;

use thiserror::Error;

use super::elevated::SUBCOMMAND;
use super::header::{HEADER_VERSION, NONCE_HEX_DIGITS};
use super::outcomes::HEADER as REPORT_HEADER;
use crate::{config, fs, plan, process};

#[derive(Debug, Error)]
pub enum Error {
    // the elevated child's gates
    #[error("refused: stdin is not a pipe; `{SUBCOMMAND}` is only for rosie itself")]
    NotAPipe,

    #[error("refused: the nonce does not match the stdin header")]
    NonceMismatch,

    #[error("refused: not running as root (uid {uid})")]
    NotRoot { uid: u32 },

    #[error("refused: no ancestor process is sudo")]
    NoSudoAncestor,

    #[error(
        "refused: tool commands never run as root; only deletes, their bootouts, and receipts are elevated"
    )]
    ToolElevated,

    // the elevation header, as the child reads it
    #[error("refused: stdin does not start with a rosie elevation header")]
    HeaderMissing,

    #[error(
        "refused: elevation header version {found} is not supported; this rosie reads version {HEADER_VERSION}"
    )]
    HeaderVersion { found: String },

    #[error("refused: elevation header line {line} is not `{expected}`")]
    HeaderMalformed { line: usize, expected: &'static str },

    #[error("refused: the elevation nonce is not {NONCE_HEX_DIGITS} lowercase hex digits")]
    InvalidNonce,

    #[error("refused: home {home:?} is not an absolute UTF-8 path on one line")]
    InvalidHome { home: PathBuf },

    #[error("refused: the elevated items are not valid UTF-8")]
    ItemsNotUtf8,

    // the elevation header, as the run writes it
    #[error("home {home:?} is not an absolute UTF-8 path on one line")]
    UnwritableHome { home: PathBuf },

    // the elevated child's report of its outcomes
    #[error("the elevated child's report is not valid UTF-8")]
    ReportNotUtf8,

    #[error("the elevated child reported nothing")]
    ReportEmpty,

    #[error("the elevated child's report starts with {0:?}; expected {REPORT_HEADER:?}")]
    ReportHeader(String),

    #[error("the elevated child's report record {record:?} is unreadable: {source}")]
    ReportParse {
        record: String,
        #[source]
        source: toml::de::Error,
    },

    #[error("the elevated child's report record {0:?} names no outcome")]
    ReportNoOutcome(String),

    #[error("the elevated child's report record names both a path and a command, or neither")]
    ReportSubject,

    #[error("cannot write the elevated report: {0}")]
    ReportSerialize(#[from] toml::ser::Error),

    #[error("cannot write the elevated report: {0}")]
    ReportWrite(#[from] io::Error),

    // what the child needs to act
    #[error(transparent)]
    Process(#[from] process::Error),

    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error(transparent)]
    Config(#[from] config::Error),

    #[error("the elevated items are not a valid plan: {0}")]
    Plan(#[from] plan::Error),
}
