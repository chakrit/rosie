use std::path::PathBuf;

use thiserror::Error;

use super::LaunchDomain;

#[derive(Debug, Error)]
pub enum Error {
    #[error("rule {rule}: cmd is empty")]
    EmptyCommand { rule: String },

    #[error("cmd is empty")]
    EmptyArgv,

    #[error("{text:?} holds a NUL byte, which no path or argument can carry")]
    NulByte { text: String },

    #[error(
        "rule {rule}: cmd {cmd:?} holds a quote or a backslash; \
         cmd is split on whitespace and never run through a shell"
    )]
    QuotedCommand { rule: String, cmd: String },

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

    #[error("[[{section}]] entry {number}: {problem}")]
    InvalidEntry {
        section: &'static str,
        /// 1-based, as a reader counts the entries in the file.
        number: usize,
        problem: String,
    },

    #[error("cannot read the plan: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("cannot write the plan: {0}")]
    Serialize(#[from] toml::ser::Error),
}
