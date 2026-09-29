//! A scan entry that is safe to claim and size: its path has no symlink in any
//! component, and it is neither a dataless placeholder nor the root of a mount, on
//! another volume than the folder holding it (`docs/spec/safety.md#symlinks`,
//! `#walk-skips`). Whether an entry is sealed that way is decided by [`Sealed::of`]
//! alone, a placeholder before a mount root, for the walk, app mode, fixed paths, and
//! sizing alike; `/`, which has no folder, is always a mount root.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::Walk;
use crate::fs::{self, Backend, FileKind, Gate, Home, Metadata};
use crate::plan::{ItemKind, WalkSkip};

/// A path with the metadata its own `lstat` gave, free of symlinks along the whole
/// path and not sealed: neither a dataless placeholder nor a mount root.
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

    #[error("{path} is the root of a mounted volume, never claimed or sized", path = path.display())]
    Mount { path: PathBuf },
}

impl Entry {
    /// `lstat`s `path` for claiming or sizing. A path through a symlink, or naming
    /// one, is refused rather than followed; a dataless placeholder is refused rather
    /// than opened, and a mount root rather than weighed as a whole volume. `/` has no
    /// folder and is the root of its volume.
    pub fn lstat<B: Backend>(gate: &Gate<B>, path: &Path) -> Result<Entry, NotAnEntry> {
        let path = fs::resolve_dots(path)?;
        let meta = gate.lstat_link_free(&path)?;
        let Some(folder) = path.parent() else {
            return Err(NotAnEntry::Mount { path });
        };
        let folder_meta = gate.lstat(folder)?;

        match Sealed::of(meta, folder_meta) {
            Some(Sealed::Placeholder) => Err(NotAnEntry::Placeholder { path }),
            Some(Sealed::Mount) => Err(NotAnEntry::Mount { path }),
            None => Ok(Entry { path, meta }),
        }
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

    /// Whether the entry, a fixed path, lies behind a volume crossing the scan may not
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

/// What one entry of a folder is, as the walk and `Folder::entry` both tell it.
#[derive(Debug)]
pub enum InFolder {
    Entry(Entry),
    Symlink,
    Sealed(PathBuf, Sealed),
}

/// Why an entry is never a target, whatever the walk settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sealed {
    Placeholder,
    /// On another volume than the folder holding it.
    Mount,
}

impl InFolder {
    /// What the entry at `path` is, from its own `lstat` and that of the folder holding
    /// it, which was `lstat`ed link-free, so every component above the entry is
    /// link-free. A symlink is told first, then a sealed entry, as [`Sealed::of`]
    /// tells it.
    pub(super) fn of(path: &Path, meta: Metadata, folder_meta: Metadata) -> InFolder {
        if meta.kind == FileKind::Symlink {
            return InFolder::Symlink;
        }
        match Sealed::of(meta, folder_meta) {
            Some(sealed) => InFolder::Sealed(path.to_path_buf(), sealed),
            None => InFolder::Entry(Entry {
                path: path.to_path_buf(),
                meta,
            }),
        }
    }
}

impl Sealed {
    /// How an entry is sealed, if it is, from its own `lstat` and that of the folder
    /// holding it: a placeholder is told before a mount root, so an entry that is both
    /// is a placeholder.
    pub(super) fn of(meta: Metadata, folder_meta: Metadata) -> Option<Sealed> {
        let mount = on_another_volume(meta, folder_meta);
        match (meta.is_dataless(), mount) {
            (true, _) => Some(Sealed::Placeholder),
            (false, true) => Some(Sealed::Mount),
            (false, false) => None,
        }
    }

    /// The walk skip a matched entry sealed this way is logged as.
    pub fn skip(self) -> WalkSkip {
        match self {
            Sealed::Placeholder => WalkSkip::SealedPlaceholder,
            Sealed::Mount => WalkSkip::SealedMount,
        }
    }
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

    /// Whether the folder, a fixed one, lies behind a volume crossing the scan may not
    /// make: see `behind_a_closed_mount`.
    pub fn is_behind_a_closed_mount<B: Backend>(
        &self,
        gate: &Gate<B>,
        home: &Home,
        mounts: Mounts,
    ) -> Result<bool, fs::Error> {
        behind_a_closed_mount(gate, &self.path, self.meta, home, mounts)
    }

    /// `lstat`s the entry `name` of this folder's listing and tells what it is as the
    /// walk does, through [`InFolder::of`]. A `name` that is not one entry's name, such
    /// as `..` or one holding `/`, is refused.
    pub(crate) fn entry<B: Backend>(
        &self,
        gate: &Gate<B>,
        name: &OsStr,
    ) -> Result<InFolder, fs::Error> {
        if Path::new(name).file_name() != Some(name) {
            let name = name.to_owned();
            return Err(fs::Error::NotAnEntryName { name });
        }
        let path = self.path.join(name);
        let meta = gate.lstat(&path)?;
        Ok(InFolder::of(&path, meta, self.meta))
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
    /// The entry is on the folder's volume, or is the root of a mount the walk enters.
    Readable(Readable<'p>),
    /// The entry is the root of a mount the walk does not enter: nothing inside it is
    /// listed or read.
    ClosedMount,
}

/// Proof that the entry at `path` is on the volume of the folder listing it, or is the
/// root of a mount the walk enters. Only [`Volume::of`] makes one, and rule detection
/// and descending both need one, so neither can reach inside a mount the walk may not
/// enter.
pub(super) struct Readable<'p> {
    path: &'p Path,
}

impl<'p> Volume<'p> {
    pub fn of(path: &'p Path, meta: Metadata, folder_meta: Metadata, mounts: Mounts) -> Self {
        let mount_root = on_another_volume(meta, folder_meta);
        match (mount_root, mounts) {
            (true, Mounts::Skip) => Volume::ClosedMount,
            (false, _) | (true, Mounts::Enter) => Volume::Readable(Readable { path }),
        }
    }
}

impl<'p> Readable<'p> {
    pub fn path(&self) -> &'p Path {
        self.path
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
    use crate::fs::SF_DATALESS;

    const FOLDER_DEV: u64 = 1;

    fn meta(kind: FileKind, dev: u64, flags: u32) -> Metadata {
        Metadata {
            kind,
            allocated: 0,
            dev,
            inode: 2,
            nlink: 1,
            uid: 501,
            mode: 0o644,
            flags,
        }
    }

    fn in_folder(meta: Metadata) -> InFolder {
        let folder = self::meta(FileKind::Dir, FOLDER_DEV, 0);
        InFolder::of(Path::new("/w/entry"), meta, folder)
    }

    fn walked(kind: FileKind) -> Entry {
        match in_folder(meta(kind, FOLDER_DEV, 0)) {
            InFolder::Entry(entry) => entry,
            other => panic!("expected an entry, got {other:?}"),
        }
    }

    #[test]
    fn only_files_and_folders_are_plan_items() {
        assert_eq!(walked(FileKind::File).item_kind(), Some(ItemKind::File));
        assert_eq!(walked(FileKind::Dir).item_kind(), Some(ItemKind::Folder));
        assert_eq!(walked(FileKind::Other).item_kind(), None);
    }

    #[test]
    fn a_placeholder_that_is_a_mount_root_is_sealed_as_a_placeholder() {
        let both = meta(FileKind::Dir, FOLDER_DEV + 1, SF_DATALESS);
        let mount = meta(FileKind::Dir, FOLDER_DEV + 1, 0);

        assert!(matches!(
            in_folder(both),
            InFolder::Sealed(_, Sealed::Placeholder)
        ));
        assert!(matches!(
            in_folder(mount),
            InFolder::Sealed(_, Sealed::Mount)
        ));
    }
}
