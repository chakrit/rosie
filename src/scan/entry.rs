//! A scan entry that is safe to claim and size: its path has no symlink in any
//! component and it is not a dataless placeholder (`docs/spec/safety.md#symlinks`,
//! `#walk-skips`). Whether an entry is a mount, on another volume than the folder
//! holding it, is decided here and nowhere else.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::Walk;
use crate::fs::{self, Backend, FileKind, Gate, Home, Metadata};
use crate::plan::ItemKind;

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

    /// What a plan records of the entry's type; nothing for a socket, fifo, or device
    /// node, which is never planned. An entry is never a symlink.
    pub fn item_kind(&self) -> Option<ItemKind> {
        match self.meta.kind {
            FileKind::File => Some(ItemKind::File),
            FileKind::Dir => Some(ItemKind::Folder),
            FileKind::Symlink | FileKind::Other => None,
        }
    }

    /// Whether the entry is a mount: on another volume than the folder holding it.
    /// `/` has no folder and is the root of its volume, so it is one.
    pub fn is_mount<B: Backend>(&self, gate: &Gate<B>) -> Result<bool, fs::Error> {
        let Some(folder) = self.path.parent() else {
            return Ok(true);
        };
        let folder_meta = gate.lstat(folder)?;
        Ok(on_another_volume(self.meta, folder_meta))
    }

    /// Whether the entry, a fixed path, lies behind a volume crossing the walk may not
    /// make: see `behind_a_closed_mount`.
    pub fn is_behind_a_closed_mount<B: Backend>(
        &self,
        gate: &Gate<B>,
        home: &Home,
        mounts: Mounts,
    ) -> Result<bool, fs::Error> {
        behind_a_closed_mount(gate, &self.path, self.meta, home, mounts)
    }

    pub(super) fn into_path(self) -> PathBuf {
        self.path
    }
}

/// A folder `lstat`ed link-free, so each entry listed in it is checked with its own
/// `lstat` alone, as the walk checks the entries of a folder it entered.
#[derive(Debug, Clone)]
pub struct Folder {
    path: PathBuf,
    meta: Metadata,
}

/// Why a path cannot be a [`Folder`].
#[derive(Debug, Error)]
pub enum NotAFolder {
    /// The `lstat` failed, or the path passes through or names a symlink.
    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error("{path} is not a folder", path = path.display())]
    Kind { path: PathBuf },
}

/// What one entry of a [`Folder`] is.
#[derive(Debug)]
pub enum InFolder {
    Entry(Entry),
    Symlink(PathBuf),
    /// On another volume than the folder.
    Mount(PathBuf),
    Placeholder(PathBuf),
}

impl Folder {
    /// `lstat`s `path` as a folder whose entries will be checked. A path through a
    /// symlink, or naming one, is refused rather than followed.
    pub fn lstat<B: Backend>(gate: &Gate<B>, path: &Path) -> Result<Folder, NotAFolder> {
        let path = fs::resolve_dots(path)?;
        let meta = gate.lstat_link_free(&path)?;
        match meta.kind {
            FileKind::Dir => Ok(Folder { path, meta }),
            FileKind::File | FileKind::Symlink | FileKind::Other => Err(NotAFolder::Kind { path }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the folder, a fixed one, lies behind a volume crossing the walk may not
    /// make: see `behind_a_closed_mount`.
    pub fn is_behind_a_closed_mount<B: Backend>(
        &self,
        gate: &Gate<B>,
        home: &Home,
        mounts: Mounts,
    ) -> Result<bool, fs::Error> {
        behind_a_closed_mount(gate, &self.path, self.meta, home, mounts)
    }

    /// `lstat`s the entry `name` of this folder's listing. A mount is told apart
    /// before a placeholder, as the walk does. A `name` that is not one entry's name,
    /// such as `..` or one holding `/`, is refused.
    pub fn entry<B: Backend>(&self, gate: &Gate<B>, name: &OsStr) -> Result<InFolder, fs::Error> {
        if Path::new(name).file_name() != Some(name) {
            let name = name.to_owned();
            return Err(fs::Error::NotAnEntryName { name });
        }
        let path = self.path.join(name);
        let meta = gate.lstat(&path)?;

        if meta.kind == FileKind::Symlink {
            return Ok(InFolder::Symlink(path));
        }
        if on_another_volume(meta, self.meta) {
            return Ok(InFolder::Mount(path));
        }
        Ok(match Entry::walked(&path, meta) {
            Some(entry) => InFolder::Entry(entry),
            None => InFolder::Placeholder(path),
        })
    }
}

/// Whether the scan crosses into another volume it meets: config `walk.enter_mounts`.
/// Even when it does, a mount point itself is never a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mounts {
    Skip,
    Enter,
}

impl From<Walk> for Mounts {
    fn from(walk: Walk) -> Self {
        match walk.enter_mounts {
            true => Mounts::Enter,
            false => Mounts::Skip,
        }
    }
}

/// Where a walked entry sits against the folder listing it, told from the two `lstat`s
/// alone, before anything reads inside the entry.
pub(super) enum Volume<'p> {
    /// The walk may read inside the entry.
    Readable(Readable<'p>),
    /// The entry is the root of a mount the walk does not enter: nothing inside it is
    /// listed or read.
    ClosedMount,
}

/// Proof that the walk may read inside the entry at `path`: it is on the volume of the
/// folder listing it, or it is the root of a mount the walk enters. Only
/// [`Volume::of`] makes one, and rule detection and descending both need one, so neither
/// can reach inside a mount the walk may not enter.
pub(super) struct Readable<'p> {
    path: &'p Path,
    mount_root: bool,
}

impl<'p> Volume<'p> {
    pub fn of(path: &'p Path, meta: Metadata, folder_meta: Metadata, mounts: Mounts) -> Self {
        let mount_root = on_another_volume(meta, folder_meta);
        match (mount_root, mounts) {
            (true, Mounts::Skip) => Volume::ClosedMount,
            (false, _) | (true, Mounts::Enter) => Volume::Readable(Readable { path, mount_root }),
        }
    }
}

impl<'p> Readable<'p> {
    pub fn path(&self) -> &'p Path {
        self.path
    }

    /// Whether the entry is the root of a mount the walk may enter. Such an entry is
    /// still never a target: the gate refuses to delete a mount point.
    pub fn is_mount_root(&self) -> bool {
        self.mount_root
    }
}

/// Whether a fixed path, whose own `lstat` gave `meta`, lies behind a volume crossing
/// the scan does not make: with [`Mounts::Skip`], the path or a folder between it and
/// its anchor is on another volume than the folder above it. The anchor is home for a
/// path under home and `/` otherwise; folders above the anchor are not looked at.
fn behind_a_closed_mount<B: Backend>(
    gate: &Gate<B>,
    path: &Path,
    meta: Metadata,
    home: &Home,
    mounts: Mounts,
) -> Result<bool, fs::Error> {
    if mounts == Mounts::Enter {
        return Ok(false);
    }

    let anchor = anchor(path, home);
    let folders = path
        .ancestors()
        .skip(1)
        .take_while(|folder| folder.starts_with(anchor));
    let folder_metas = folders
        .map(|folder| gate.lstat(folder))
        .collect::<Result<Vec<_>, _>>()?;
    let metas: Vec<Metadata> = std::iter::once(meta).chain(folder_metas).collect();

    Ok(metas
        .windows(2)
        .any(|pair| on_another_volume(pair[0], pair[1])))
}

/// Where the volume check for a fixed path starts: home for a path under it, however
/// the rule spelled it, and `/` otherwise. Both `path` and home have their dots
/// resolved, so each has one spelling.
fn anchor<'p>(path: &Path, home: &'p Home) -> &'p Path {
    match path.starts_with(home) {
        true => home,
        false => Path::new("/"),
    }
}

/// Whether an entry is on another volume than the folder holding it: the root of a
/// mount.
fn on_another_volume(entry: Metadata, folder: Metadata) -> bool {
    entry.dev != folder.dev
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walked(kind: FileKind) -> Entry {
        let meta = Metadata {
            kind,
            allocated: 0,
            dev: 1,
            inode: 2,
            nlink: 1,
            uid: 501,
            mode: 0o644,
            flags: 0,
        };
        Entry::walked(Path::new("/w/entry"), meta).expect("not a symlink or placeholder")
    }

    #[test]
    fn only_files_and_folders_are_plan_items() {
        assert_eq!(walked(FileKind::File).item_kind(), Some(ItemKind::File));
        assert_eq!(walked(FileKind::Dir).item_kind(), Some(ItemKind::Folder));
        assert_eq!(walked(FileKind::Other).item_kind(), None);
    }
}
