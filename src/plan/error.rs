use std::path::PathBuf;

use thiserror::Error;

use super::{ItemKind, LaunchDomain, StatusKind};
use crate::fs::InvalidCommand;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{text:?} holds a NUL byte, which no path or argument can carry")]
    NulByte { text: String },

    #[error("{plist} is booted out of both {first} and {second}", plist = plist.display())]
    ConflictingLaunchDomain {
        plist: PathBuf,
        first: LaunchDomain,
        second: LaunchDomain,
    },

    #[error("{path:?} is not absolute")]
    RelativePath { path: PathBuf },

    #[error("{path:?} is not valid UTF-8, so a plan file cannot hold it")]
    NonUtf8Path { path: PathBuf },

    #[error("{path:?} holds a . or .. component, which a plan path never holds")]
    DotComponent { path: PathBuf },

    #[error("package is missing")]
    EmptyPackage,

    #[error("subject is missing")]
    EmptySubject,

    #[error("the plan has no format version; it was not written by rosie scan")]
    MissingVersion,

    #[error("plan format version {found} is not supported; this rosie reads version {supported}")]
    UnsupportedVersion { found: i64, supported: u32 },

    #[error(
        "delete {inner:?} is inside delete {outer:?}; a scan never writes this, so run \
         cannot tell which order is safe",
        inner = inner.display().to_string(),
        outer = outer.display().to_string()
    )]
    NestedDeletes { outer: PathBuf, inner: PathBuf },

    #[error(
        "delete {path:?} is listed twice; a scan never writes this",
        path = path.display().to_string()
    )]
    RepeatedDelete { path: PathBuf },

    #[error("the plan has {count} rule conflict(s)")]
    UnresolvedConflicts { count: usize },

    #[error("[[{section}]] entry {number}: {problem}")]
    InvalidEntry {
        section: &'static str,
        /// 1-based, as a reader counts the entries in the file.
        number: usize,
        problem: Box<EntryProblem>,
    },

    #[error("cannot read the plan: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("cannot write the plan: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// Why one entry of a plan file cannot be read back.
#[derive(Debug, Error)]
pub enum EntryProblem {
    #[error("{field} {source}")]
    Path {
        field: &'static str,
        #[source]
        source: Error,
    },

    #[error(
        "type {text:?} is not one of {}",
        ItemKind::ALL.map(ItemKind::label).join(", ")
    )]
    UnknownKind { text: String },

    #[error("size is missing")]
    MissingSize,

    #[error(
        "status {text:?} is not one of {}",
        StatusKind::ALL.map(StatusKind::label).join(", ")
    )]
    UnknownStatus { text: String },

    #[error(
        "status {text:?} is a rule conflict; resolve it to {} or {}",
        StatusKind::Clean.label(),
        StatusKind::Skip.label()
    )]
    ConflictStatus { text: String },

    #[error(
        "status {text:?} is not one of {}, {}",
        StatusKind::Clean.label(),
        StatusKind::Skip.label()
    )]
    UnknownSelection { text: String },

    #[error(
        "bootout plist {:?} is not inside path {:?}",
        plist.display().to_string(),
        path.display().to_string()
    )]
    PlistOutsideDelete { plist: PathBuf, path: PathBuf },

    #[error("bootout domain {text:?} is not system or gui/<uid>")]
    UnknownDomain { text: String },

    #[error("bootout plist {:?} is listed twice", plist.display().to_string())]
    RepeatedPlist { plist: PathBuf },

    #[error("shell is missing")]
    MissingShell,

    #[error(transparent)]
    Command(InvalidCommand),

    #[error(transparent)]
    Value(Error),
}
