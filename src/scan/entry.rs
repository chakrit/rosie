//! A scan entry that is safe to claim and size: its path has no symlink in any
//! component and it is not a dataless placeholder (`docs/spec/safety.md#symlinks`,
//! `#walk-skips`).

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::fs::{self, Backend, FileKind, Gate, Metadata};

/// A path with the metadata its own `lstat` gave, free of symlinks along the whole
/// path and not a dataless placeholder.
#[derive(Debug, Clone)]
pub struct Entry {
    path: PathBuf,
    meta: Metadata,
}

/// Why a path cannot be an [`Entry`].
#[derive(Debug, Error)]
pub enum NotAnEntry {
    /// The `lstat` failed, or the path passes through or names a symlink.
    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error("{path} is a dataless placeholder, left unopened", path = path.display())]
    Placeholder { path: PathBuf },
}

impl Entry {
    /// `lstat`s `path` for claiming or sizing. A path through a symlink, or naming
    /// one, is refused rather than followed, and a dataless placeholder is refused
    /// rather than opened.
    pub fn lstat<B: Backend>(gate: &Gate<B>, path: &Path) -> Result<Entry, NotAnEntry> {
        let path = fs::resolve_dots(path)?;
        let meta = gate.lstat_link_free(&path)?;
        match meta.is_dataless() {
            true => Err(NotAnEntry::Placeholder { path }),
            false => Ok(Entry { path, meta }),
        }
    }

    /// An entry the walk met in a folder it had itself `lstat`ed as a folder, so
    /// every component above it is link-free; nothing when the entry is a symlink or
    /// a placeholder.
    pub(super) fn walked(path: &Path, meta: Metadata) -> Option<Entry> {
        let claimable = meta.kind != FileKind::Symlink && !meta.is_dataless();
        claimable.then(|| Entry {
            path: path.to_path_buf(),
            meta,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn meta(&self) -> Metadata {
        self.meta
    }

    pub(super) fn into_path(self) -> PathBuf {
        self.path
    }
}
