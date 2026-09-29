//! What root may delete on another user's behalf.
//!
//! Root acts on paths, and the kernel follows a symlink in any folder of a path. So
//! root enters a folder only when no one but root can change it: a folder that
//! another user could write could have an entry swapped for a symlink between root's
//! `lstat` of it and root's use of it, redirecting root's unlink elsewhere.
//!
//! The invariant, held by induction from `/` down:
//!
//! - every folder above the item is a folder (never a symlink), owned by root, and not
//!   writable by group or others, except that a folder above the item's parent may be
//!   sticky and writable by others, as `/private/tmp` is: others cannot rename root's
//!   entries in it;
//! - the item's parent, the item when it is a folder, and every folder inside the item
//!   are root-owned folders that neither group nor others can write, sticky or not.
//!
//! Each folder is checked by `lstat` before it is entered. Its `lstat` result is stable
//! because its parent already passed: only root can rename or replace an entry of a
//! root-controlled folder. Root never changes a folder's permissions; a root-controlled
//! tree needs none changed to be deleted.
//!
//! Mode bits only: an ACL granting another user write access to a root-owned folder is
//! not seen.

use std::fmt;
use std::path::Path;

use super::Gate;
use crate::fs::backend::{Backend, FileKind, Metadata, ROOT_UID};
use crate::fs::error::{Error, Op};

const GROUP_OR_OTHER_WRITE: u32 = 0o022;
const STICKY: u32 = 0o1000;

/// How someone other than root could change a folder root would enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exposure {
    /// It is not a folder, such as a symlink standing in for one.
    NotAFolder(FileKind),
    NotRootOwned {
        uid: u32,
    },
    WritableByOthers,
}

impl fmt::Display for Exposure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Exposure::NotAFolder(kind) => write!(f, "is a {}", kind.label()),
            Exposure::NotRootOwned { uid } => write!(f, "is owned by uid {uid}, not root"),
            Exposure::WritableByOthers => f.write_str("is writable by non-root users"),
        }
    }
}

/// Where a folder lies relative to the item root deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Above the item's parent: sticky and writable by others passes.
    AboveParent,
    /// The item's parent, the item, or inside it.
    ParentOrBelow,
}

impl<B: Backend> Gate<B> {
    /// Checks every folder from `/` down to the parent of the resolved `path`, by
    /// `lstat`. A folder that cannot be inspected fails the check.
    pub(super) fn require_root_controlled_above(&self, path: &Path) -> Result<(), Error> {
        let mut folders: Vec<&Path> = path.ancestors().skip(1).collect();
        folders.reverse();
        let parent = folders.last().copied();

        for folder in folders {
            let place = match Some(folder) == parent {
                true => Place::ParentOrBelow,
                false => Place::AboveParent,
            };
            let meta = self
                .io(Op::Lstat, folder, self.backend.lstat(folder))
                .map_err(|error| Error::AboveUnchecked(Box::new(error)))?;
            require(folder, meta, place)?;
        }
        Ok(())
    }
}

/// Checks a folder root is about to enter: the item itself, or a folder inside it.
pub(super) fn require_root_controlled(folder: &Path, meta: Metadata) -> Result<(), Error> {
    require(folder, meta, Place::ParentOrBelow)
}

fn require(folder: &Path, meta: Metadata, place: Place) -> Result<(), Error> {
    match exposure(meta, place) {
        None => Ok(()),
        Some(exposure) => Err(Error::NotRootControlled {
            folder: folder.to_path_buf(),
            exposure,
        }),
    }
}

fn exposure(meta: Metadata, place: Place) -> Option<Exposure> {
    if meta.kind != FileKind::Dir {
        return Some(Exposure::NotAFolder(meta.kind));
    }
    if meta.uid != ROOT_UID {
        return Some(Exposure::NotRootOwned { uid: meta.uid });
    }

    let writable = meta.mode & GROUP_OR_OTHER_WRITE != 0;
    let sticky = meta.mode & STICKY != 0;
    match (writable, sticky, place) {
        (false, _, _) => None,
        (true, true, Place::AboveParent) => None,
        (true, _, _) => Some(Exposure::WritableByOthers),
    }
}
