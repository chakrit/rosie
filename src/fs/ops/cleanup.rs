//! Ops on the trees rosie scans and the cleanup targets it deletes
//! (`docs/spec/safety.md#cleanup-targets`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{FsOp, sealed};
use crate::fs::backend::{Backend, Metadata};
use crate::fs::context::FsContext;
use crate::fs::error::Error;
use crate::fs::links::EnterLinks;

/// The owner of every op on scanned trees and cleanup targets.
pub enum Cleanup {}

impl Cleanup {
    /// Lists a folder.
    pub fn read_dir(path: &Path) -> ReadDir<'_> {
        ReadDir { path }
    }

    /// `lstat`s a path, never following a symlink at its end.
    pub fn lstat(path: &Path) -> Lstat<'_> {
        Lstat { path }
    }

    /// `lstat`s a path, refusing a symlink in any component, the last included, rather
    /// than following it, such as a file a Lua rule tests for or a fixed leftover
    /// folder app mode lists.
    pub fn lstat_link_free(path: &Path) -> LstatLinkFree<'_> {
        LstatLinkFree { path }
    }

    /// Reads a file of a scanned tree, such as a marker a Lua rule inspects, refusing a
    /// symlink in any component rather than following it.
    pub fn read_file(path: &Path) -> ReadFile<'_> {
        ReadFile { path }
    }

    /// Where an absolute rule fixed path is on disk, as [`crate::fs::UserInput::path`]
    /// locates a typed one: a symlink in it is refused unless `links` allows one.
    pub fn locate(path: &Path, links: EnterLinks) -> Locate<'_> {
        Locate { path, links }
    }

    /// Deletes a file, folder tree, or other entry. Performing it refuses the item while
    /// a process runs a program from inside it, outside the roots, through a symlink, or
    /// on a mount point, and then deletes it as whoever the context acts as.
    pub fn delete(path: &Path) -> Delete<'_> {
        Delete { path }
    }
}

pub struct ReadDir<'p> {
    path: &'p Path,
}

pub struct Lstat<'p> {
    path: &'p Path,
}

pub struct Locate<'p> {
    path: &'p Path,
    links: EnterLinks,
}

pub struct LstatLinkFree<'p> {
    path: &'p Path,
}

pub struct ReadFile<'p> {
    path: &'p Path,
}

pub struct Delete<'p> {
    path: &'p Path,
}

impl Delete<'_> {
    /// Runs every check `perform` runs, without deleting, so work that must only happen
    /// for a delete that will go ahead, such as booting out the launch job whose plist
    /// it is, is decided first. `perform` checks again.
    pub fn verify<B: Backend>(&self, ctx: &FsContext<B>) -> Result<(), Error> {
        ctx.admit_delete(self.path).map(|_| ())
    }
}

impl sealed::Sealed for ReadDir<'_> {}
impl sealed::Sealed for Lstat<'_> {}
impl sealed::Sealed for LstatLinkFree<'_> {}
impl sealed::Sealed for ReadFile<'_> {}
impl sealed::Sealed for Locate<'_> {}
impl sealed::Sealed for Delete<'_> {}

impl<B: Backend> FsOp<B> for ReadDir<'_> {
    type Output = Vec<OsString>;

    fn perform(self, ctx: &FsContext<B>) -> Result<Vec<OsString>, Error> {
        ctx.read_dir(self.path)
    }
}

impl<B: Backend> FsOp<B> for Lstat<'_> {
    type Output = Metadata;

    fn perform(self, ctx: &FsContext<B>) -> Result<Metadata, Error> {
        ctx.lstat(self.path)
    }
}

impl<B: Backend> FsOp<B> for LstatLinkFree<'_> {
    type Output = Metadata;

    fn perform(self, ctx: &FsContext<B>) -> Result<Metadata, Error> {
        ctx.lstat_link_free(self.path)
    }
}

impl<B: Backend> FsOp<B> for ReadFile<'_> {
    type Output = Vec<u8>;

    fn perform(self, ctx: &FsContext<B>) -> Result<Vec<u8>, Error> {
        ctx.read_file(self.path)
    }
}

impl<B: Backend> FsOp<B> for Locate<'_> {
    type Output = PathBuf;

    fn perform(self, ctx: &FsContext<B>) -> Result<PathBuf, Error> {
        ctx.locate(self.path, self.links)
    }
}

impl<B: Backend + Sync> FsOp<B> for Delete<'_> {
    type Output = ();

    fn perform(self, ctx: &FsContext<B>) -> Result<(), Error> {
        let admitted = ctx.admit_delete(self.path)?;
        ctx.delete(admitted)
    }
}
