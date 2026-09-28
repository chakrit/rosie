//! `Backend` for the machine, on std only: no `libc`, no FFI.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write as _};
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use super::backend::{Argv, Backend, CommandOutput, Exit, FileKind, Metadata};

/// `st_blocks` counts 512-byte units regardless of the volume's block size.
const BLOCK_UNIT: u64 = 512;

#[derive(Debug, Default, Clone, Copy)]
pub struct RealBackend;

impl Backend for RealBackend {
    fn read_dir(&self, path: &Path) -> io::Result<Vec<OsString>> {
        fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect()
    }

    fn lstat(&self, path: &Path) -> io::Result<Metadata> {
        let meta = fs::symlink_metadata(path)?;
        Ok(Metadata {
            kind: kind_of(meta.file_type()),
            allocated: meta.blocks() * BLOCK_UNIT,
            dev: meta.dev(),
            inode: meta.ino(),
            uid: meta.uid(),
            mode: meta.mode() & 0o7777,
            flags: meta.st_flags(),
        })
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        fs::read_link(path)
    }

    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn write_file(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        fs::write(path, contents)
    }

    fn create_file(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(contents)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn remove_empty_dir(&self, path: &Path) -> io::Result<()> {
        fs::remove_dir(path)
    }

    fn set_mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn run(&self, argv: &Argv) -> io::Result<CommandOutput> {
        let output = Command::new(argv.program())
            .args(argv.args())
            .stdin(Stdio::null())
            .output()?;

        Ok(CommandOutput {
            exit: exit_of(output.status)?,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

fn kind_of(file_type: fs::FileType) -> FileKind {
    if file_type.is_symlink() {
        FileKind::Symlink
    } else if file_type.is_dir() {
        FileKind::Dir
    } else if file_type.is_file() {
        FileKind::File
    } else {
        FileKind::Other
    }
}

fn exit_of(status: ExitStatus) -> io::Result<Exit> {
    let code = status.code().map(Exit::Code);
    let signal = status.signal().map(Exit::Signal);
    code.or(signal).ok_or_else(|| {
        io::Error::other(format!("process ended without a code or signal: {status}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_a_program_without_a_shell_and_captures_its_output() {
        let argv = Argv::new("/bin/echo").arg("$HOME;").arg("two words");

        let output = RealBackend.run(&argv).expect("run echo");

        assert_eq!(output.exit, Exit::Code(0));
        assert_eq!(output.stdout, b"$HOME; two words\n");
    }

    #[test]
    fn reports_a_failing_exit_code() {
        let output = RealBackend
            .run(&Argv::new("/usr/bin/false"))
            .expect("run false");

        assert_eq!(output.exit, Exit::Code(1));
        assert!(!output.exit.success());
    }
}
