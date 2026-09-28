//! `Backend` for the machine, on std only: no `libc`, no FFI.

use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead as _, BufReader, Read, Write};
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, ScopedJoinHandle};

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

    fn run_streaming<F: FnMut(&str)>(&self, argv: &Argv, mut on_line: F) -> io::Result<Exit> {
        let mut child = Command::new(argv.program())
            .args(argv.args())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| missing_pipe("stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| missing_pipe("stderr"))?;

        let (lines, received) = mpsc::channel();
        thread::scope(|scope| {
            let out = scope.spawn({
                let lines = lines.clone();
                move || forward_lines(stdout, lines)
            });
            let err = scope.spawn(move || forward_lines(stderr, lines));
            for line in received {
                on_line(&line);
            }
            joined(out)?;
            joined(err)
        })?;

        exit_of(child.wait()?)
    }

    fn sudo(&self, argv: &Argv, stdin: &[u8]) -> io::Result<CommandOutput> {
        let mut child = Command::new("sudo")
            .arg(argv.program())
            .args(argv.args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let pipe = child.stdin.take().ok_or_else(|| missing_pipe("stdin"))?;

        let output = thread::scope(|scope| {
            let feeder = scope.spawn(move || feed(pipe, stdin));
            let output = child.wait_with_output();
            joined(feeder)?;
            output
        })?;

        Ok(CommandOutput {
            exit: exit_of(output.status)?,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

/// Sends each line `source` produces, without its line ending, until it ends.
fn forward_lines(source: impl Read, lines: Sender<String>) -> io::Result<()> {
    let mut reader = BufReader::new(source);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        if reader.read_until(b'\n', &mut buffer)? == 0 {
            return Ok(());
        }

        let line = String::from_utf8_lossy(&buffer);
        let line = line.trim_end_matches(['\n', '\r']).to_owned();
        lines
            .send(line)
            .map_err(|_| io::Error::other("command output listener went away"))?;
    }
}

/// Writes `bytes` to a child's stdin and closes it. A child that exits without reading
/// everything, such as sudo refusing the password, breaks the pipe; its exit status is
/// what reports that, so the broken pipe itself is not an error.
fn feed(mut pipe: impl Write, bytes: &[u8]) -> io::Result<()> {
    match pipe.write_all(bytes) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        written => written,
    }
}

fn joined(handle: ScopedJoinHandle<'_, io::Result<()>>) -> io::Result<()> {
    handle
        .join()
        .map_err(|_| io::Error::other("command pipe thread panicked"))?
}

fn missing_pipe(name: &str) -> io::Error {
    io::Error::other(format!("the command's {name} was not piped"))
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
    fn streams_stdout_and_stderr_lines_as_they_arrive() {
        let argv = Argv::new("/bin/sh")
            .arg("-c")
            .arg("echo one; echo two >&2; printf three; exit 3");
        let mut lines = Vec::new();

        let exit = RealBackend
            .run_streaming(&argv, |line| lines.push(line.to_owned()))
            .expect("run sh");

        lines.sort();
        assert_eq!(exit, Exit::Code(3));
        assert_eq!(lines, vec!["one", "three", "two"]);
    }

    #[test]
    fn feeding_a_child_that_stopped_reading_is_not_an_error() {
        let (reader, writer) = io::pipe().expect("pipe");
        drop(reader);

        let fed = feed(writer, b"items sudo never read");

        assert!(fed.is_ok(), "{fed:?}");
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
