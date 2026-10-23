//! Ops on rosie's own config and data, confined to the injected `~/.config/rosie` and
//! `~/.local/share/rosie`, not checked against `roots`, and refused through a symlink in
//! any component (`docs/spec/safety.md#rosies-own-data`).

use std::path::{Path, PathBuf};

use super::{FsOp, sealed};
use crate::fs::backend::{Backend, DirEntry, FileKind, Metadata};
use crate::fs::context::FsContext;
use crate::fs::error::Error;

/// The owner of every op on rosie's own data.
pub enum Rosie {}

impl Rosie {
    /// Lists a folder under rosie's own folders.
    pub fn read_dir(path: &Path) -> ReadDir<'_> {
        ReadDir { path }
    }

    /// Lists a folder under rosie's own folders: every entry with its kind, in name
    /// order, whatever the backend's own order; none when the folder is missing. The
    /// folder is `lstat`ed, with no symlink anywhere on its path, before it is listed,
    /// and refused when it is not a folder.
    pub fn entries(path: &Path) -> Entries<'_> {
        Entries { path }
    }

    /// `lstat`s an entry under rosie's own folders, such as one found listing a folder. A
    /// symlink above it is refused; one at the entry itself is reported as a symlink,
    /// never followed.
    pub fn lstat(path: &Path) -> Lstat<'_> {
        Lstat { path }
    }

    /// `lstat`s a path under rosie's own folders, refusing a symlink in any component,
    /// the last included, such as a folder about to be listed.
    pub fn lstat_link_free(path: &Path) -> LstatLinkFree<'_> {
        LstatLinkFree { path }
    }

    /// Reads a file under rosie's own folders.
    pub fn read(path: &Path) -> Read<'_> {
        Read { path }
    }

    /// Creates or replaces a file under rosie's own folders.
    pub fn write<'p>(path: &'p Path, contents: &'p [u8]) -> Write<'p> {
        Write { path, contents }
    }

    /// Creates a new file under rosie's own folders. Fails with `AlreadyExists` when
    /// the volume already holds the name under any spelling it treats as the same.
    pub fn create<'p>(path: &'p Path, contents: &'p [u8]) -> Create<'p> {
        Create { path, contents }
    }

    /// Creates a folder under rosie's own folders, with any missing parents.
    pub fn create_dir_all(path: &Path) -> CreateDirAll<'_> {
        CreateDirAll { path }
    }

    /// Moves an entry within rosie's own folders, such as a freshly unpacked pack into
    /// its final place. The own folders themselves are never moved or replaced.
    pub fn rename<'p>(from: &'p Path, to: &'p Path) -> Rename<'p> {
        Rename { from, to }
    }

    /// Deletes an entry inside rosie's own folders, one entry at a time, such as a
    /// removed pack. The own folders themselves are never deleted.
    pub fn delete(path: &Path) -> Delete<'_> {
        Delete { path }
    }
}

pub struct ReadDir<'p> {
    path: &'p Path,
}

pub struct Entries<'p> {
    path: &'p Path,
}

pub struct Lstat<'p> {
    path: &'p Path,
}

pub struct LstatLinkFree<'p> {
    path: &'p Path,
}

pub struct Read<'p> {
    path: &'p Path,
}

pub struct Write<'p> {
    path: &'p Path,
    contents: &'p [u8],
}

pub struct Create<'p> {
    path: &'p Path,
    contents: &'p [u8],
}

pub struct CreateDirAll<'p> {
    path: &'p Path,
}

pub struct Rename<'p> {
    from: &'p Path,
    to: &'p Path,
}

pub struct Delete<'p> {
    path: &'p Path,
}

impl sealed::Sealed for ReadDir<'_> {}
impl sealed::Sealed for Entries<'_> {}
impl sealed::Sealed for Lstat<'_> {}
impl sealed::Sealed for LstatLinkFree<'_> {}
impl sealed::Sealed for Read<'_> {}
impl sealed::Sealed for Write<'_> {}
impl sealed::Sealed for Create<'_> {}
impl sealed::Sealed for CreateDirAll<'_> {}
impl sealed::Sealed for Rename<'_> {}
impl sealed::Sealed for Delete<'_> {}

impl<B: Backend> FsOp<B> for ReadDir<'_> {
    type Output = Vec<DirEntry>;

    fn perform(self, ctx: &FsContext<B>) -> Result<Vec<DirEntry>, Error> {
        ctx.read_own_dir(self.path)
    }
}

impl<B: Backend> FsOp<B> for Entries<'_> {
    type Output = Vec<(PathBuf, FileKind)>;

    fn perform(self, ctx: &FsContext<B>) -> Result<Vec<(PathBuf, FileKind)>, Error> {
        match Rosie::lstat_link_free(self.path).perform(ctx) {
            Ok(meta) if meta.kind == FileKind::Dir => {}
            Ok(meta) => {
                return Err(Error::NotAFolder {
                    path: self.path.into(),
                    found: meta.kind.label(),
                });
            }
            Err(error) if error.is_not_found() => return Ok(Vec::new()),
            Err(error) => return Err(error),
        }

        let mut listed = Rosie::read_dir(self.path).perform(ctx)?;
        listed.sort_by(|a, b| a.name.cmp(&b.name));
        listed
            .into_iter()
            .map(|entry| {
                let path = self.path.join(entry.name);
                let meta = Rosie::lstat(&path).perform(ctx)?;
                Ok((path, meta.kind))
            })
            .collect()
    }
}

impl<B: Backend> FsOp<B> for Lstat<'_> {
    type Output = Metadata;

    fn perform(self, ctx: &FsContext<B>) -> Result<Metadata, Error> {
        ctx.lstat_own(self.path)
    }
}

impl<B: Backend> FsOp<B> for LstatLinkFree<'_> {
    type Output = Metadata;

    fn perform(self, ctx: &FsContext<B>) -> Result<Metadata, Error> {
        ctx.lstat_own_link_free(self.path)
    }
}

impl<B: Backend> FsOp<B> for Read<'_> {
    type Output = Vec<u8>;

    fn perform(self, ctx: &FsContext<B>) -> Result<Vec<u8>, Error> {
        ctx.read_own_file(self.path)
    }
}

impl<B: Backend> FsOp<B> for Write<'_> {
    type Output = ();

    fn perform(self, ctx: &FsContext<B>) -> Result<(), Error> {
        ctx.write_own_file(self.path, self.contents)
    }
}

impl<B: Backend> FsOp<B> for Create<'_> {
    type Output = ();

    fn perform(self, ctx: &FsContext<B>) -> Result<(), Error> {
        ctx.create_own_file(self.path, self.contents)
    }
}

impl<B: Backend> FsOp<B> for CreateDirAll<'_> {
    type Output = ();

    fn perform(self, ctx: &FsContext<B>) -> Result<(), Error> {
        ctx.create_own_dir_all(self.path)
    }
}

impl<B: Backend> FsOp<B> for Rename<'_> {
    type Output = ();

    fn perform(self, ctx: &FsContext<B>) -> Result<(), Error> {
        ctx.rename_own(self.from, self.to)
    }
}

impl<B: Backend> FsOp<B> for Delete<'_> {
    type Output = ();

    fn perform(self, ctx: &FsContext<B>) -> Result<(), Error> {
        ctx.delete_own(self.path)
    }
}
