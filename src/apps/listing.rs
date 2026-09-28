//! Listing the fixed folders app scans search: leftover locations and app folders.

use std::ffi::OsString;
use std::path::Path;

use crate::fs::{self, Backend, FileKind, Gate};

/// What a fixed folder holds.
pub(super) enum Listing {
    Entries(Vec<OsString>),
    /// Nothing is there, or not a folder: there is nothing to search.
    NoFolder,
    /// The folder exists but rosie may not list it.
    Denied(fs::Error),
}

/// Lists a fixed folder. A symlink in any component of it is refused, as for any fixed
/// path (`docs/spec/safety.md#symlinks`).
pub(super) fn list_folder<B: Backend>(gate: &Gate<B>, path: &Path) -> Result<Listing, fs::Error> {
    let meta = match gate.lstat_without_symlinks(path) {
        Ok(meta) => meta,
        Err(error) if error.has_vanished() => return Ok(Listing::NoFolder),
        Err(error) => return Err(error),
    };
    if meta.kind != FileKind::Dir {
        return Ok(Listing::NoFolder);
    }

    match gate.read_dir(path) {
        Ok(names) => Ok(Listing::Entries(names)),
        Err(error) if error.is_permission_denied() => Ok(Listing::Denied(error)),
        Err(error) => Err(error),
    }
}
