//! Running processes, read from `ps -axo pid=,ppid=,uid=,comm=`
//! (`docs/spec/safety.md#running-processes`).
//!
//! Every plan, in every mode, refuses items a process is executing from; the `comm`
//! path is matched as written against item paths, component by component. Bare names
//! and symlinked launches are not matched.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::fs::{self, Argv, Backend, Exit, Gate};

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error("`{argv}` failed ({exit:?}): {stderr}")]
    Failed {
        argv: Argv,
        exit: Exit,
        stderr: String,
    },

    #[error("`{argv}` printed output that is not UTF-8")]
    NotUtf8 { argv: Argv },

    #[error("`ps` printed a line rosie cannot read: {line:?}")]
    Malformed { line: String },
}

/// One process as `ps` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    /// The executable as `comm` shows it: an absolute path, or a bare name.
    pub command: PathBuf,
}

impl Process {
    /// Whether this process executes from `path` or from inside it. A bare name never
    /// matches.
    pub fn executes_from(&self, path: &Path) -> bool {
        self.command.is_absolute() && self.command.starts_with(path)
    }
}

pub(crate) fn argv() -> Argv {
    Argv::new("ps").arg("-axo").arg("pid=,ppid=,uid=,comm=")
}

/// Lists the running processes.
pub fn list<B: Backend>(gate: &Gate<B>) -> Result<Vec<Process>, Error> {
    let argv = argv();
    let output = gate.run(&argv)?;
    if !output.exit.success() {
        return Err(Error::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit: output.exit,
            argv,
        });
    }

    let text = String::from_utf8(output.stdout).map_err(|_| Error::NotUtf8 { argv })?;
    parse(&text)
}

/// Parses `ps -axo pid=,ppid=,uid=,comm=` output. `comm` is the rest of the line, so it
/// keeps any spaces in the executable's path.
fn parse(text: &str) -> Result<Vec<Process>, Error> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_line)
        .collect()
}

fn parse_line(line: &str) -> Result<Process, Error> {
    let malformed = || Error::Malformed {
        line: line.to_owned(),
    };

    let (pid, rest) = next_field(line).ok_or_else(malformed)?;
    let (ppid, rest) = next_field(rest).ok_or_else(malformed)?;
    let (uid, command) = next_field(rest).ok_or_else(malformed)?;
    if command.is_empty() {
        return Err(malformed());
    }

    let id = |field: &str| field.parse::<u32>().map_err(|_| malformed());
    // `ps` prints a uid outside `int32` range, such as `nobody` (4294967294), as a
    // negative number: parse it signed and reinterpret the bits as unsigned.
    let signed_id = |field: &str| field.parse::<i32>().map_err(|_| malformed());
    Ok(Process {
        pid: id(pid)?,
        ppid: id(ppid)?,
        uid: signed_id(uid)?.cast_unsigned(),
        command: PathBuf::from(command),
    })
}

/// The first whitespace-separated field of `text` and what follows it, with the
/// whitespace between them removed.
fn next_field(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    let end = text.find(char::is_whitespace)?;
    Some((&text[..end], text[end..].trim_start()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::fake::{FakeBackend, USER_UID};
    use crate::fs::{Bounds, CommandOutput};

    fn process(command: &str) -> Process {
        Process {
            pid: 1,
            ppid: 0,
            uid: 501,
            command: PathBuf::from(command),
        }
    }

    #[test]
    fn parses_pid_ppid_uid_and_a_command_with_spaces() {
        let text = "  355     1   501 /Applications/Foo Bar.app/Contents/MacOS/Foo Bar\n\
                    \n    1     0     0 /sbin/launchd\n";

        let processes = parse(text).expect("valid ps output");

        assert_eq!(
            processes,
            [
                Process {
                    pid: 355,
                    ppid: 1,
                    uid: 501,
                    command: PathBuf::from("/Applications/Foo Bar.app/Contents/MacOS/Foo Bar"),
                },
                Process {
                    pid: 1,
                    ppid: 0,
                    uid: 0,
                    command: PathBuf::from("/sbin/launchd"),
                },
            ]
        );
    }

    #[test]
    fn refuses_a_line_without_every_field() {
        let result = parse("  355     1   501\n");

        assert!(matches!(result, Err(Error::Malformed { line }) if line.contains("355")));
    }

    #[test]
    fn refuses_a_line_whose_command_is_blank() {
        let result = parse("  355     1   501 \n");

        assert!(matches!(result, Err(Error::Malformed { line }) if line.contains("355")));
    }

    #[test]
    fn refuses_a_non_numeric_id() {
        for line in [
            "  abc     1   501 /bin/zsh\n",
            "  355   abc   501 /bin/zsh\n",
            "  355     1   abc /bin/zsh\n",
        ] {
            let result = parse(line);

            assert!(matches!(result, Err(Error::Malformed { .. })), "{line:?}");
        }
    }

    /// macOS `ps` prints the uid of a user outside `int32` range, such as `nobody`
    /// (4294967294), as a negative number: real output seen starting `dhcp6d` on a stock
    /// Mac.
    #[test]
    fn a_negative_uid_is_read_as_its_unsigned_value() {
        let processes = parse("51253     1    -2 /usr/libexec/dhcp6d\n").expect("valid ps output");

        assert_eq!(processes[0].uid, 4294967294);
    }

    #[test]
    fn a_process_executes_from_its_folder_by_component() {
        let foo = process("/Applications/Foo.app/Contents/MacOS/Foo");

        assert!(foo.executes_from(Path::new("/Applications/Foo.app")));
        assert!(foo.executes_from(Path::new("/Applications/Foo.app/Contents/MacOS/Foo")));
        assert!(!foo.executes_from(Path::new("/Applications/Fo")));
        assert!(
            !process("/Applications/Foo.application/x")
                .executes_from(Path::new("/Applications/Foo.app"))
        );
    }

    #[test]
    fn a_bare_name_matches_nothing() {
        assert!(!process("Foo").executes_from(Path::new("Foo")));
    }

    // listing

    fn gate(fake: &FakeBackend) -> Gate<&FakeBackend> {
        let bounds = Bounds {
            roots: vec![PathBuf::from("/w")],
            config_dir: PathBuf::from("/c"),
            data_dir: PathBuf::from("/d"),
            user_uid: USER_UID,
        };
        Gate::new(fake, bounds).expect("absolute bounds")
    }

    fn ps_prints(fake: &FakeBackend, exit: i32, stdout: &[u8]) {
        let output = CommandOutput {
            exit: Exit::Code(exit),
            stdout: stdout.to_vec(),
            stderr: b"ps: broken\n".to_vec(),
        };
        fake.respond(argv(), output);
    }

    /// The fake answers by this same argv, so only this test pins the columns `parse`
    /// reads.
    #[test]
    fn ps_is_asked_for_pid_ppid_uid_and_comm_without_headers() {
        assert_eq!(argv().to_string(), "ps -axo pid=,ppid=,uid=,comm=");
    }

    #[test]
    fn lists_the_processes_ps_prints() {
        let fake = FakeBackend::new();
        ps_prints(&fake, 0, b"    1     0     0 /sbin/launchd\n");

        let processes = list(&gate(&fake)).expect("listing");

        assert_eq!(
            processes,
            [Process {
                pid: 1,
                ppid: 0,
                uid: 0,
                command: PathBuf::from("/sbin/launchd")
            }]
        );
    }

    #[test]
    fn a_ps_that_cannot_be_run_fails_the_listing() {
        let fake = FakeBackend::new();

        let result = list(&gate(&fake));

        assert!(
            matches!(result, Err(Error::Fs(fs::Error::Command { .. }))),
            "{result:?}"
        );
    }

    #[test]
    fn a_failing_ps_fails_the_listing() {
        let fake = FakeBackend::new();
        ps_prints(&fake, 1, b"    1     0     0 /sbin/launchd\n");

        let result = list(&gate(&fake));

        assert!(matches!(result, Err(Error::Failed { .. })), "{result:?}");
    }

    #[test]
    fn ps_output_that_is_not_utf8_fails_the_listing() {
        let fake = FakeBackend::new();
        ps_prints(&fake, 0, b"    1     0     0 /sbin/launch\xff\n");

        let result = list(&gate(&fake));

        assert!(matches!(result, Err(Error::NotUtf8 { .. })), "{result:?}");
    }
}
