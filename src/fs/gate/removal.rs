//! The delete engines behind `Gate::delete`, `Gate::delete_as_root`, and
//! `Gate::delete_own` (`docs/spec/safety.md#deletion`). Callers have already checked
//! the top path; every path below it comes from a listing and is `lstat`ed, never
//! followed.
//!
//! Both engines share one step, [`Gate::open_dir_for_removal`]; they differ only in
//! whether a folder's entries are removed one after another or in parallel
//! (`docs/spec/performance.md#run`). How a folder is entered depends on whom the
//! delete acts as: see [`Entering`].

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use super::Gate;
use super::root_control::require_root_controlled;
use crate::fs::backend::{Backend, FileKind, Metadata};
use crate::fs::error::{Error, Op};

/// Owner read, write, and search: what removing a folder's entries needs.
const OWNER_RWX: u32 = 0o700;

/// How a delete enters each folder it removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Entering {
    /// Acting as the user: the user's own closed folders are opened on the way.
    OpeningUsersFolders,
    /// Acting as root: only folders no one but root can change are entered, and no
    /// permissions are changed.
    RootControlledOnly,
}

impl<B: Backend> Gate<B> {
    /// Removes an entry and everything below it, one entry at a time, as the user.
    pub(super) fn remove_entry(&self, path: &Path, meta: Metadata) -> Result<(), Error> {
        refuse_placeholder(path, meta)?;
        match meta.kind {
            FileKind::Dir => {
                let entering = Entering::OpeningUsersFolders;
                for (child, child_meta) in self.open_dir_for_removal(path, meta, entering)? {
                    self.remove_entry(&child, child_meta)?;
                }
                self.io(Op::RemoveDir, path, self.backend.remove_empty_dir(path))
            }
            FileKind::File | FileKind::Symlink | FileKind::Other => {
                self.io(Op::RemoveFile, path, self.backend.remove_file(path))
            }
        }
    }

    /// Enters a folder to remove it and lists its entries with their metadata. An
    /// entry on another volume is refused rather than descended into.
    fn open_dir_for_removal(
        &self,
        path: &Path,
        meta: Metadata,
        entering: Entering,
    ) -> Result<Vec<(PathBuf, Metadata)>, Error> {
        match entering {
            Entering::OpeningUsersFolders => self.open_for_removal(path, meta)?,
            Entering::RootControlledOnly => require_root_controlled(path, meta)?,
        }

        let names = self.io(Op::ReadDir, path, self.backend.read_dir(path))?;
        names
            .into_iter()
            .map(|name| {
                let child = path.join(name);
                let child_meta = self.io(Op::Lstat, &child, self.backend.lstat(&child))?;
                match child_meta.dev == meta.dev {
                    true => Ok((child, child_meta)),
                    false => Err(Error::CrossesVolume { path: child }),
                }
            })
            .collect()
    }

    /// Makes a user-owned folder listable and writable so its entries can be removed.
    /// Other users' folders are left as they are.
    fn open_for_removal(&self, path: &Path, meta: Metadata) -> Result<(), Error> {
        let owned = meta.uid == self.user_uid;
        let open = meta.mode & OWNER_RWX == OWNER_RWX;
        if !owned || open {
            return Ok(());
        }

        let mode = meta.mode | OWNER_RWX;
        self.io(Op::SetMode, path, self.backend.set_mode(path, mode))
    }
}

impl<B: Backend + Sync> Gate<B> {
    /// Removes an entry and everything below it, the entries of each folder in
    /// parallel. A folder is removed once all its entries are.
    pub(super) fn remove_entry_in_parallel(
        &self,
        path: &Path,
        meta: Metadata,
        entering: Entering,
    ) -> Result<(), Error> {
        refuse_placeholder(path, meta)?;
        match meta.kind {
            FileKind::Dir => {
                self.open_dir_for_removal(path, meta, entering)?
                    .par_iter()
                    .try_for_each(|(child, child_meta)| {
                        self.remove_entry_in_parallel(child, *child_meta, entering)
                    })?;
                self.io(Op::RemoveDir, path, self.backend.remove_empty_dir(path))
            }
            FileKind::File | FileKind::Symlink | FileKind::Other => {
                self.io(Op::RemoveFile, path, self.backend.remove_file(path))
            }
        }
    }
}

/// Listing a dataless folder downloads it, and unlinking a dataless file deletes it from
/// the cloud too; rosie does neither.
fn refuse_placeholder(path: &Path, meta: Metadata) -> Result<(), Error> {
    match meta.is_dataless() {
        true => Err(Error::Placeholder {
            path: path.to_path_buf(),
        }),
        false => Ok(()),
    }
}
