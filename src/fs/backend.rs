//! The `Backend` trait: the raw primitives the FS interaction layer is built on.
//!
//! A backend performs each primitive exactly as asked. It knows nothing of roots, symlink
//! bans, or rosie's own folders; those live in [`FsContext`](super::FsContext), the only thing the
//! rest of rosie receives. See `docs/spec/safety.md#fs-interaction-layer`.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

/// `SF_DATALESS` from `<sys/stat.h>`: the file is a cloud placeholder whose data is not on
/// disk. Opening it triggers a download.
pub const SF_DATALESS: u32 = 0x4000_0000;

/// `ELOOP` from `<sys/errno.h>` on macOS: a path walk met more symlinks than the kernel
/// follows, as a loop of links does.
pub const ELOOP: i32 = 62;

/// Filesystem, command, and metadata primitives.
///
/// No `Send` or `Sync` bound: only parallel functions add `where B: Sync`.
pub trait Backend {
    /// Names of the entries in a folder, excluding `.` and `..`, in no particular order.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<OsString>>;

    /// Metadata of the path itself, never of a symlink's target.
    fn lstat(&self, path: &Path) -> io::Result<Metadata>;

    /// The path with every symlink followed, `.` and `..` applied, and each name spelled
    /// as stored on disk (`realpath`). Fails when the path does not exist.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;

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

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        (**self).canonicalize(path)
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

/// A program and its arguments, run without a shell, with variables added to rosie's
/// environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argv {
    program: OsString,
    args: Vec<OsString>,
    envs: Vec<(OsString, OsString)>,
}

impl Argv {
    pub fn new(program: impl Into<OsString>) -> Self {
        Argv {
            program: program.into(),
            args: Vec::new(),
            envs: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn env(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.envs.push((name.into(), value.into()));
        self
    }

    pub fn program(&self) -> &OsStr {
        &self.program
    }

    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    pub fn envs(&self) -> &[(OsString, OsString)] {
        &self.envs
    }
}

/// `NAME=value program arg…`, as `env` would read it.
impl std::fmt::Display for Argv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (name, value) in &self.envs {
            write!(f, "{}={} ", name.to_string_lossy(), value.to_string_lossy())?;
        }
        write!(f, "{}", self.program.to_string_lossy())?;
        for arg in &self.args {
            write!(f, " {}", arg.to_string_lossy())?;
        }
        Ok(())
    }
}

/// A system tool rosie runs by its absolute path, so no `PATH` lookup can put another
/// program in its place, in the elevated child least of all. Every tool is one of the
/// constants below, and the constructor they share rejects a relative path while rosie
/// compiles. A tool rule names its own shell ([`crate::fs::ShellCommand`]), whose command
/// line is meant to use `PATH`; only the default one, bash, is a system tool.
///
/// Each tool carries where its version is read and the version rosie was tested with
/// (`docs/spec/safety.md#system-tool-versions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemTool {
    path: &'static str,
    version: VersionSource,
}

impl SystemTool {
    pub const BASH: SystemTool = SystemTool::at(
        "/bin/bash",
        VersionSource::Own {
            args: &["--version"],
            format: VersionFormat::Bash,
            tested: "3.2.57",
        },
    );
    pub const ID: SystemTool = SystemTool::at("/usr/bin/id", VersionSource::MacOs);
    pub const LAUNCHCTL: SystemTool = SystemTool::at(
        "/bin/launchctl",
        VersionSource::Own {
            args: &["version"],
            format: VersionFormat::Launchctl,
            tested: "7.0.0",
        },
    );
    pub const LSOF: SystemTool = SystemTool::at(
        "/usr/sbin/lsof",
        VersionSource::Own {
            args: &["-v"],
            format: VersionFormat::Lsof,
            tested: "4.91",
        },
    );
    pub const PKGUTIL: SystemTool = SystemTool::at("/usr/sbin/pkgutil", VersionSource::MacOs);
    pub const PLUTIL: SystemTool = SystemTool::at("/usr/bin/plutil", VersionSource::MacOs);
    pub const PS: SystemTool = SystemTool::at("/bin/ps", VersionSource::MacOs);
    pub const SUDO: SystemTool = SystemTool::at(
        "/usr/bin/sudo",
        VersionSource::Own {
            args: &["-V"],
            format: VersionFormat::Sudo,
            tested: "1.9.17p2",
        },
    );
    pub const SW_VERS: SystemTool = SystemTool::at("/usr/bin/sw_vers", VersionSource::IsMacOs);

    const ALL: [SystemTool; 9] = [
        SystemTool::BASH,
        SystemTool::ID,
        SystemTool::LAUNCHCTL,
        SystemTool::LSOF,
        SystemTool::PKGUTIL,
        SystemTool::PLUTIL,
        SystemTool::PS,
        SystemTool::SUDO,
        SystemTool::SW_VERS,
    ];

    const fn at(path: &'static str, version: VersionSource) -> SystemTool {
        assert!(
            matches!(path.as_bytes(), [b'/', ..]),
            "a system tool is named by its absolute path"
        );
        SystemTool { path, version }
    }

    /// The system tool a program path names, if it names one.
    pub fn at_path(program: &OsStr) -> Option<SystemTool> {
        SystemTool::ALL
            .into_iter()
            .find(|tool| OsStr::new(tool.path) == program)
    }

    pub const fn path(self) -> &'static str {
        self.path
    }

    pub fn version(self) -> VersionSource {
        self.version
    }

    /// The tool's command line, with no arguments yet.
    pub fn argv(self) -> SystemArgv {
        SystemArgv(Argv::new(self.path))
    }
}

/// Where a system tool's version is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionSource {
    /// The tool reports its own version when run with `args`.
    Own {
        args: &'static [&'static str],
        format: VersionFormat,
        tested: &'static str,
    },
    /// The tool reports no version; the macOS version stands for it.
    MacOs,
    /// The tool that reports the macOS version.
    IsMacOs,
}

/// How a tool's own version output reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionFormat {
    /// `GNU bash, version 3.2.57(1)-release …`
    Bash,
    /// `Darwin Bootstrapper Version 7.0.0: …`
    Launchctl,
    /// `lsof version information:` then `    revision: 4.91`
    Lsof,
    /// `Sudo version 1.9.17p2`
    Sudo,
}

/// A command line whose program is a [`SystemTool`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemArgv(Argv);

impl SystemArgv {
    pub fn arg(self, arg: impl Into<OsString>) -> Self {
        SystemArgv(self.0.arg(arg))
    }

    pub fn as_argv(&self) -> &Argv {
        &self.0
    }
}

impl From<SystemArgv> for Argv {
    fn from(argv: SystemArgv) -> Argv {
        argv.0
    }
}

impl std::fmt::Display for SystemArgv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
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
