//! `FsContext<B>`: the one wrapper every other part of rosie receives instead of a backend.
//! Every filesystem access is a typed op from [`crate::fs::ops`], performed against a
//! context, which holds what the op's owner checks it by
//! (`docs/spec/safety.md#fs-interaction-layer`):
//!
//! - Cleanup-target deletes are refused outside the injected `roots`, through any
//!   symlink, on a mount point, and while a process runs from inside the path
//!   (`docs/spec/safety.md#cleanup-targets`, `#symlinks`, `#running-processes`).
//! - Rosie's own data is confined to the injected config and data folders, not checked
//!   against roots (`docs/spec/safety.md#rosies-own-data`).
//! - Commands pass through; errors name their path or command.

mod admission;
mod canonical;
mod lexical;
mod removal;
mod root_control;
mod running;

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::backend::{Argv, Backend, CommandOutput, Exit, Metadata};
use super::error::{Error, Op};
use super::paths::Paths;
use running::Running;

pub use lexical::resolve_dots;
pub use root_control::Exposure;
pub use running::running_argv;

/// Everything a context is bounded by, injected by the caller and never read from the
/// environment.
#[derive(Debug, Clone)]
pub struct Bounds {
    /// Folders that cleanup-target deletes must lie within.
    pub roots: Vec<PathBuf>,
    /// The home folder and rosie's own folders under it.
    pub paths: Paths,
    /// The user rosie acts for; only their read-only folders are made writable to delete.
    pub user_uid: u32,
}

/// Whom the process holding a context runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acting {
    User,
    /// The elevated child, acting as root on the user's behalf.
    Root,
}

/// The FS interaction layer's one wrapper, mostly for security gates: it carries the
/// roots, rosie's own folders, whom the process acts as, and the running-program
/// snapshot that every op checks against. Code only ever receives a context, never a
/// bare backend.
pub struct FsContext<B: Backend> {
    backend: B,
    roots: Vec<PathBuf>,
    paths: Paths,
    own_dirs: [PathBuf; 2],
    user_uid: u32,
    acting: Acting,
    /// Taken by the first check that needs it, so once per scan or run; a failure is
    /// kept as its message and refuses every delete.
    running: OnceLock<Result<Running, String>>,
}

impl<B: Backend> FsContext<B> {
    /// A context acting as the user.
    pub fn new(backend: B, bounds: Bounds) -> Result<Self, Error> {
        Self::acting_as(backend, bounds, Acting::User)
    }

    /// A context for the elevated child, acting as root on the user's behalf.
    pub fn new_as_root(backend: B, bounds: Bounds) -> Result<Self, Error> {
        Self::acting_as(backend, bounds, Acting::Root)
    }

    fn acting_as(backend: B, bounds: Bounds, acting: Acting) -> Result<Self, Error> {
        let roots = bounds
            .roots
            .iter()
            .map(|root| resolve_dots(root))
            .collect::<Result<Vec<_>, _>>()?;
        let own_dirs = [bounds.paths.config_dir(), bounds.paths.data_dir()];

        Ok(FsContext {
            backend,
            roots,
            paths: bounds.paths,
            own_dirs,
            user_uid: bounds.user_uid,
            acting,
            running: OnceLock::new(),
        })
    }

    // what the context holds

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// The user rosie acts for.
    pub fn user_uid(&self) -> u32 {
        self.user_uid
    }

    pub fn acting(&self) -> Acting {
        self.acting
    }

    /// Whether a delete of `path` passes the roots check: `.` and `..` resolved by text,
    /// then compared component by component with every root
    /// (`docs/spec/safety.md#cleanup-targets`). The disk is not consulted.
    pub fn within_roots(&self, path: &Path) -> Result<bool, Error> {
        let path = resolve_dots(path)?;
        Ok(self.contains(&path))
    }

    /// Takes the running-program snapshot now, if no check has taken it yet, so a scan
    /// that cannot read it fails before it starts rather than refusing each item.
    pub fn snapshot_running(&self) -> Result<(), Error> {
        self.running().map(|_| ())
    }

    // commands

    /// Runs a tool command. Commands are outside the roots check; the plan lists each
    /// for confirmation (`docs/spec/safety.md#cleanup-targets`).
    pub fn run(&self, argv: &Argv) -> Result<CommandOutput, Error> {
        self.backend.run(argv).map_err(|source| Error::Command {
            argv: argv.clone(),
            source,
        })
    }

    /// Runs a tool command like [`FsContext::run`], handing each line of its output to
    /// `on_line` as it arrives.
    pub fn run_streaming<F: FnMut(&str)>(&self, argv: &Argv, on_line: F) -> Result<Exit, Error> {
        self.backend
            .run_streaming(argv, on_line)
            .map_err(|source| Error::Command {
                argv: argv.clone(),
                source,
            })
    }

    /// Runs `sudo <argv>` with plain `sudo`, piping `stdin` to it
    /// (`docs/spec/safety.md#elevation`).
    pub fn sudo(&self, argv: &Argv, stdin: &[u8]) -> Result<CommandOutput, Error> {
        self.backend
            .sudo(argv, stdin)
            .map_err(|source| Error::Command {
                argv: argv.clone(),
                source,
            })
    }

    // reads, for the ops

    pub(in crate::fs) fn read_dir(&self, path: &Path) -> Result<Vec<OsString>, Error> {
        let path = resolve_dots(path)?;
        self.io(Op::ReadDir, &path, self.backend.read_dir(&path))
    }

    pub(in crate::fs) fn lstat(&self, path: &Path) -> Result<Metadata, Error> {
        let path = resolve_dots(path)?;
        self.io(Op::Lstat, &path, self.backend.lstat(&path))
    }

    /// `lstat`s a path, refusing a symlink in any component, the last included, rather
    /// than following it; a missing path is an `Io` error naming it.
    pub(in crate::fs) fn lstat_link_free(&self, path: &Path) -> Result<Metadata, Error> {
        let path = resolve_dots(path)?;
        self.lstat_real(&path)
    }

    /// Reads a file, refusing a symlink in any component rather than following it.
    pub(in crate::fs) fn read_file(&self, path: &Path) -> Result<Vec<u8>, Error> {
        let path = self.check_real(path)?;
        self.io(Op::ReadFile, &path, self.backend.read_file(&path))
    }

    /// Reads a file the user named, following any symlink on its way.
    pub(in crate::fs) fn read_file_through_links(&self, path: &Path) -> Result<Vec<u8>, Error> {
        let path = resolve_dots(path)?;
        self.io(Op::ReadFile, &path, self.backend.read_file(&path))
    }

    // rosie's own data

    // Every own-data path is refused through a symlink in any component, the own
    // folders and their ancestors included (`docs/spec/safety.md#symlinks`).

    pub(in crate::fs) fn read_own_dir(&self, path: &Path) -> Result<Vec<OsString>, Error> {
        let path = self.confine_to_own(path)?;
        self.check_real(&path)?;
        self.io(Op::ReadDir, &path, self.backend.read_dir(&path))
    }

    /// A symlink above the entry is refused; one at the entry itself is reported as a
    /// symlink, never followed.
    pub(in crate::fs) fn lstat_own(&self, path: &Path) -> Result<Metadata, Error> {
        let path = self.confine_to_own(path)?;
        if let Some(folder) = path.parent() {
            self.check_real(folder)?;
        }
        self.io(Op::Lstat, &path, self.backend.lstat(&path))
    }

    pub(in crate::fs) fn lstat_own_link_free(&self, path: &Path) -> Result<Metadata, Error> {
        let path = self.confine_to_own(path)?;
        self.lstat_real(&path)
    }

    pub(in crate::fs) fn read_own_file(&self, path: &Path) -> Result<Vec<u8>, Error> {
        let path = self.confine_to_own(path)?;
        self.check_real(&path)?;
        self.io(Op::ReadFile, &path, self.backend.read_file(&path))
    }

    pub(in crate::fs) fn write_own_file(&self, path: &Path, contents: &[u8]) -> Result<(), Error> {
        let path = self.confine_to_own(path)?;
        self.check_real(&path)?;
        self.io(
            Op::WriteFile,
            &path,
            self.backend.write_file(&path, contents),
        )
    }

    /// Fails with `AlreadyExists` when the volume already holds the name under any
    /// spelling it treats as the same.
    pub(in crate::fs) fn create_own_file(&self, path: &Path, contents: &[u8]) -> Result<(), Error> {
        let path = self.confine_to_own(path)?;
        self.check_real(&path)?;
        self.io(
            Op::CreateFile,
            &path,
            self.backend.create_file(&path, contents),
        )
    }

    pub(in crate::fs) fn create_own_dir_all(&self, path: &Path) -> Result<(), Error> {
        let path = self.confine_to_own(path)?;
        self.check_real(&path)?;
        self.io(Op::CreateDir, &path, self.backend.create_dir_all(&path))
    }

    /// The own folders themselves are never moved or replaced.
    pub(in crate::fs) fn rename_own(&self, from: &Path, to: &Path) -> Result<(), Error> {
        let from = self.confine_below_own(from)?;
        let to = self.confine_below_own(to)?;
        self.lstat_real(&from)?;
        self.check_real(&to)?;
        self.io(Op::Rename, &from, self.backend.rename(&from, &to))
    }

    /// The own folders themselves are never deleted.
    pub(in crate::fs) fn delete_own(&self, path: &Path) -> Result<(), Error> {
        let path = self.confine_below_own(path)?;
        let meta = self.lstat_real(&path)?;
        self.remove_item(&path, meta)
    }

    // bounds

    fn confine_to_roots(&self, path: &Path) -> Result<PathBuf, Error> {
        let path = resolve_dots(path)?;
        match self.contains(&path) {
            true => Ok(path),
            false => Err(Error::OutsideRoots { path }),
        }
    }

    /// Whether an already resolved path lies within a root.
    fn contains(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root))
    }

    fn confine_to_own(&self, path: &Path) -> Result<PathBuf, Error> {
        let path = resolve_dots(path)?;
        match self.own_dirs.iter().any(|dir| path.starts_with(dir)) {
            true => Ok(path),
            false => Err(Error::OutsideOwnData { path }),
        }
    }

    /// Confines to an entry strictly inside one of rosie's own folders.
    fn confine_below_own(&self, path: &Path) -> Result<PathBuf, Error> {
        let path = self.confine_to_own(path)?;
        match self.own_dirs.contains(&path) {
            true => Err(Error::OwnFolderItself { path }),
            false => Ok(path),
        }
    }

    // running programs

    /// Refuses an already resolved path while a process runs a program from inside it
    /// (`docs/spec/safety.md#running-processes`).
    fn refuse_running(&self, path: &Path) -> Result<(), Error> {
        match self.running()?.inside(path) {
            Some(program) => Err(Error::Running {
                path: path.to_path_buf(),
                pid: program.pid,
                program: program.path,
            }),
            None => Ok(()),
        }
    }

    fn running(&self) -> Result<&Running, Error> {
        let snapshot = self.running.get_or_init(|| self.read_running());
        snapshot.as_ref().map_err(|reason| Error::RunningUnknown {
            reason: reason.clone(),
        })
    }

    fn read_running(&self) -> Result<Running, String> {
        let argv = running_argv();
        let output = self
            .run(argv.as_argv())
            .map_err(|error| error.to_string())?;
        match output.exit.success() {
            true => Running::parse(&output.stdout),
            false => Err(format!(
                "`{argv}` failed ({:?}): {}",
                output.exit,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        }
    }

    /// Attaches the operation and path to a backend error.
    fn io<T>(&self, op: Op, path: &Path, result: io::Result<T>) -> Result<T, Error> {
        result.map_err(|source| Error::Io {
            op,
            path: path.to_path_buf(),
            source,
        })
    }
}

#[cfg(test)]
mod tests;
