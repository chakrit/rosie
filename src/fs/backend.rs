//! The `Backend` trait: the raw primitives the FS interaction layer is built on.
//!
//! A backend performs each primitive exactly as asked. It knows nothing of roots, symlink
//! bans, or rosie's own folders; those live in [`Gate`](super::Gate), the only thing the
//! rest of rosie receives. See `docs/spec/safety.md#fs-interaction-layer`.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

/// `SF_DATALESS` from `<sys/stat.h>`: the file is a cloud placeholder whose data is not on
/// disk. Opening it triggers a download.
pub const SF_DATALESS: u32 = 0x4000_0000;

/// Filesystem, command, and metadata primitives.
///
/// No `Send` or `Sync` bound: only parallel functions add `where B: Sync`.
pub trait Backend {
    /// Names of the entries in a folder, excluding `.` and `..`, in no particular order.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<OsString>>;

    /// Metadata of the path itself, never of a symlink's target.
    fn lstat(&self, path: &Path) -> io::Result<Metadata>;

    /// The target a symlink points at, as stored in the link.
    fn read_link(&self, path: &Path) -> io::Result<PathBuf>;

    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Creates or truncates a regular file with the given contents.
    fn write_file(&self, path: &Path, contents: &[u8]) -> io::Result<()>;

    /// Creates a new regular file with the given contents (`O_CREAT | O_EXCL`). Fails
    /// with `AlreadyExists` when any entry, a symlink included, already holds the name
    /// as the volume compares names: case and Unicode form folded on a default APFS
    /// volume. A final symlink is never followed.
    fn create_file(&self, path: &Path, contents: &[u8]) -> io::Result<()>;

    /// Creates a folder and any missing parents.
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;

    /// Unlinks a file or a symlink. A symlink is removed as a link; its target is untouched.
    fn remove_file(&self, path: &Path) -> io::Result<()>;

    /// Removes an empty folder.
    fn remove_empty_dir(&self, path: &Path) -> io::Result<()>;

    /// Sets the permission bits (`0o7777` mask) of a file or folder.
    fn set_mode(&self, path: &Path, mode: u32) -> io::Result<()>;

    /// Renames within one volume, replacing an empty destination folder or a file.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;

    /// Runs a program directly, never through a shell, with stdin closed, and waits for
    /// it to finish.
    fn run(&self, argv: &Argv) -> io::Result<CommandOutput>;

    /// Runs a program like [`Backend::run`], handing each line of its stdout and stderr
    /// to `on_line` as it arrives instead of collecting them.
    fn run_streaming<F: FnMut(&str)>(&self, argv: &Argv, on_line: F) -> io::Result<Exit>;

    /// Runs `sudo <argv>` with plain `sudo`, no flags. `stdin` is piped to it and then
    /// closed; its stdout is collected. Its stderr is rosie's own, where sudo asks for
    /// the password and the elevated child reports its progress.
    fn sudo(&self, argv: &Argv, stdin: &[u8]) -> io::Result<CommandOutput>;
}

impl<B: Backend + ?Sized> Backend for &B {
    fn read_dir(&self, path: &Path) -> io::Result<Vec<OsString>> {
        (**self).read_dir(path)
    }

    fn lstat(&self, path: &Path) -> io::Result<Metadata> {
        (**self).lstat(path)
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        (**self).read_link(path)
    }

    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        (**self).read_file(path)
    }

    fn write_file(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        (**self).write_file(path, contents)
    }

    fn create_file(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        (**self).create_file(path, contents)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        (**self).create_dir_all(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        (**self).remove_file(path)
    }

    fn remove_empty_dir(&self, path: &Path) -> io::Result<()> {
        (**self).remove_empty_dir(path)
    }

    fn set_mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        (**self).set_mode(path, mode)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        (**self).rename(from, to)
    }

    fn run(&self, argv: &Argv) -> io::Result<CommandOutput> {
        (**self).run(argv)
    }

    fn run_streaming<F: FnMut(&str)>(&self, argv: &Argv, on_line: F) -> io::Result<Exit> {
        (**self).run_streaming(argv, on_line)
    }

    fn sudo(&self, argv: &Argv, stdin: &[u8]) -> io::Result<CommandOutput> {
        (**self).sudo(argv, stdin)
    }
}

/// The root user's id.
pub const ROOT_UID: u32 = 0;

/// What `lstat` reports about one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metadata {
    pub kind: FileKind,
    /// Bytes allocated on disk (`st_blocks * 512`), not the logical length.
    pub allocated: u64,
    pub dev: u64,
    pub inode: u64,
    /// Names the entry has (`st_nlink`). For a folder, the volume's own count: on APFS,
    /// 2 plus the entries in it.
    pub nlink: u64,
    pub uid: u32,
    /// Permission bits (`st_mode & 0o7777`).
    pub mode: u32,
    /// BSD file flags (`st_flags`).
    pub flags: u32,
}

impl Metadata {
    pub fn is_dataless(&self) -> bool {
        self.flags & SF_DATALESS != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Dir,
    Symlink,
    /// Sockets, fifos, and device nodes.
    Other,
}

impl FileKind {
    pub fn label(self) -> &'static str {
        match self {
            FileKind::File => "file",
            FileKind::Dir => "folder",
            FileKind::Symlink => "symlink",
            FileKind::Other => "special file",
        }
    }
}

/// A program and its arguments, run without a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argv {
    program: OsString,
    args: Vec<OsString>,
}

impl Argv {
    pub fn new(program: impl Into<OsString>) -> Self {
        Argv {
            program: program.into(),
            args: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn program(&self) -> &OsStr {
        &self.program
    }

    pub fn args(&self) -> &[OsString] {
        &self.args
    }
}

impl std::fmt::Display for Argv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.program.to_string_lossy())?;
        for arg in &self.args {
            write!(f, " {}", arg.to_string_lossy())?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub exit: Exit,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// How a finished process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
}

impl Exit {
    pub fn success(self) -> bool {
        self == Exit::Code(0)
    }
}
