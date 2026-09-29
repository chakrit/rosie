//! Listing the fixed folders app scans search: leftover locations and app folders.

use std::ffi::OsString;
use std::path::Path;

use crate::fs::{self, Backend, Gate};
use crate::scan::{Folder, NotAFolder};

/// What a fixed folder holds.
pub(super) enum Listing {
    /// The folder's entry names.
    Entries(Vec<OsString>),
    /// Nothing is there, or not a folder: there is nothing to search.
    NoFolder,
    /// The folder exists but rosie may not list it.
    Denied(fs::Error),
}

/// Lists a fixed folder. A symlink in any component of it is refused, as for any fixed
/// path (`docs/spec/safety.md#symlinks`).
pub(super) fn list_folder<B: Backend>(gate: &Gate<B>, path: &Path) -> Result<Listing, fs::Error> {
    let Some(folder) = find_folder(gate, path)? else {
        return Ok(Listing::NoFolder);
    };

    Ok(match list_found(gate, &folder)? {
        Listed::Names(names) => Listing::Entries(names),
        Listed::Denied(error) => Listing::Denied(error),
    })
}

/// `lstat`s a fixed folder without listing it; nothing when nothing is there or it is
/// not a folder. A symlink in any component of it is refused.
pub(super) fn find_folder<B: Backend>(
    gate: &Gate<B>,
    path: &Path,
) -> Result<Option<Folder>, fs::Error> {
    match Folder::lstat(gate, path) {
        Ok(folder) => Ok(Some(folder)),
        Err(NotAFolder::Kind { .. }) => Ok(None),
        Err(NotAFolder::Fs(error)) if error.has_vanished() => Ok(None),
        Err(NotAFolder::Fs(error)) => Err(error),
    }
}

/// What listing a folder that is there gave.
pub(super) enum Listed {
    Names(Vec<OsString>),
    /// Rosie may not list the folder.
    Denied(fs::Error),
}

/// Lists a folder [`find_folder`] found.
pub(super) fn list_found<B: Backend>(gate: &Gate<B>, folder: &Folder) -> Result<Listed, fs::Error> {
    match gate.read_dir(folder.path()) {
        Ok(names) => Ok(Listed::Names(names)),
        Err(error) if error.is_permission_denied() => Ok(Listed::Denied(error)),
        Err(error) => Err(error),
    }
}
