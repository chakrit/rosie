use std::io;
use std::path::PathBuf;

use thiserror::Error;

use super::source::Source;
use crate::{fs, rules};

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error("`{text}` is not a GitHub owner/repo whose names are valid pack names")]
    BadSource { text: String },

    #[error("`{text}` names a pack `user`, which is reserved for your own rules")]
    ReservedPack { text: String },

    #[error("cannot download {url}: {source}")]
    Download {
        url: String,
        #[source]
        source: io::Error,
    },

    #[error("the pack archive cannot be read: {0}")]
    Archive(#[source] io::Error),

    #[error("the pack archive entry {path} is refused: {refusal}", path = path.display(), refusal = refusal.label())]
    Unsafe { path: PathBuf, refusal: Refusal },

    #[error("the pack archive entry {path} is not inside the archive's single top folder", path = path.display())]
    OutsideTopFolder { path: PathBuf },

    #[error("the pack archive holds {path} twice", path = path.display())]
    Duplicate { path: PathBuf },

    #[error("the pack archive entry {path} is larger than {limit} bytes", path = path.display())]
    TooLarge { path: PathBuf, limit: u64 },

    #[error("the pack archive unpacks to more than {limit} bytes")]
    UnpackedTooLarge { limit: u64 },

    #[error("the pack archive has no rules/*.toml files")]
    NoRules,

    #[error("the pack archive has no config.toml at its root")]
    NoConfig,

    #[error("the default config.toml is not UTF-8 text")]
    ConfigNotText,

    #[error("the pack's rules are invalid: {0}")]
    InvalidRules(#[source] Box<rules::Error>),

    #[error(
        "{error}; moving the previous pack back also failed ({restore}); it is kept at {set_aside} and rosie puts it back on its next run",
        set_aside = set_aside.display(),
    )]
    Restore {
        #[source]
        error: Box<fs::Error>,
        restore: Box<fs::Error>,
        set_aside: PathBuf,
    },

    #[error("the pull time {path} is unreadable; run rosie rules pull to refresh the pack", path = path.display())]
    BadPullTime { path: PathBuf },

    #[error(
        "the pack {pack} is already installed from {installed}; remove it first with rosie rules remove {installed_pack}",
        installed_pack = installed.pack(),
    )]
    PackNameTaken { pack: String, installed: Source },

    #[error(
        "the volume stores the owner {owner} under {path}, which is not a valid owner name; remove that entry first",
        path = path.display(),
    )]
    StrayOwnerFolder { owner: String, path: PathBuf },

    #[error("no pack named {pack} is installed")]
    NotInstalled { pack: String },

    #[error("several installed packs are named {pack}: {sources}", sources = join(sources))]
    Ambiguous { pack: String, sources: Vec<Source> },
}

/// Why an archive entry is refused (`docs/spec/safety.md#rosies-own-data`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Absolute,
    ParentDir,
    Symlink,
    Hardlink,
    Device,
    /// Fifos, sparse files, and every other entry type that is not a regular file or a
    /// folder.
    Special,
}

impl Refusal {
    pub fn label(self) -> &'static str {
        match self {
            Refusal::Absolute => "absolute path",
            Refusal::ParentDir => "`..` in its path",
            Refusal::Symlink => "symlink",
            Refusal::Hardlink => "hardlink",
            Refusal::Device => "device node",
            Refusal::Special => "not a regular file or folder",
        }
    }
}

fn join(sources: &[Source]) -> String {
    sources
        .iter()
        .map(Source::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
