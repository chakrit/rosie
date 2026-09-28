//! The sequential delete engine behind `Gate::delete` and `Gate::delete_own`
//! (`docs/spec/safety.md#deletion`). Callers have already checked the top path; every
//! path below it comes from a listing and is `lstat`ed, never followed.

use std::path::Path;

use super::Gate;
use crate::fs::backend::{Backend, FileKind, Metadata};
use crate::fs::error::{Error, Op};

/// Owner read, write, and search: what removing a folder's entries needs.
const OWNER_RWX: u32 = 0o700;

impl<B: Backend> Gate<B> {
    pub(super) fn remove_entry(&self, path: &Path, meta: Metadata) -> Result<(), Error> {
        match meta.kind {
            FileKind::Dir => self.remove_dir_tree(path, meta),
            FileKind::File | FileKind::Symlink | FileKind::Other => {
                self.io(Op::RemoveFile, path, self.backend.remove_file(path))
            }
        }
    }

    fn remove_dir_tree(&self, path: &Path, meta: Metadata) -> Result<(), Error> {
        self.open_for_removal(path, meta)?;

        let names = self.io(Op::ReadDir, path, self.backend.read_dir(path))?;
        for name in names {
            let child = path.join(name);
            let child_meta = self.io(Op::Lstat, &child, self.backend.lstat(&child))?;
            if child_meta.dev != meta.dev {
                return Err(Error::CrossesVolume { path: child });
            }
            self.remove_entry(&child, child_meta)?;
        }

        self.io(Op::RemoveDir, path, self.backend.remove_empty_dir(path))
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
