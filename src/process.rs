//! Facts about running processes and the current user, read from system CLIs through
//! the gate's command method (`docs/spec/safety.md#running-processes`,
//! `docs/decisions/2026-09-29-elevation-without-ffi.md`).
//!
//! Shared by the scanner, app mode, and the runner: all of them refuse items a process
//! executes from, and the elevated entry walks the parent chain to find `sudo`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::fs::{self, Argv, Backend, CommandOutput, Gate};

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error("`{argv}` failed: {stderr}")]
    Failed { argv: Argv, stderr: String },

    #[error("cannot read `{argv}` output line {line:?}")]
    Unreadable { argv: Argv, line: String },
}

/// One row of `ps -axo pid=,ppid=,uid=,comm=`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    /// `ps` prints this as `uid_t` reinterpreted signed, so `nobody` (-2) parses as a
    /// negative number rather than failing the row.
    pub uid: i32,
    /// The executable as `ps` prints it: a full path, or a bare name.
    pub comm: PathBuf,
}

/// Every process running when the table was read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcessTable {
    processes: Vec<Process>,
}

impl ProcessTable {
    pub fn argv() -> Argv {
        Argv::new("/bin/ps")
            .arg("-axo")
            .arg("pid=,ppid=,uid=,comm=")
    }

    /// Reads the table with `ps`.
    pub fn query<B: Backend>(gate: &Gate<B>) -> Result<Self, Error> {
        let argv = Self::argv();
        let output = succeeded(&argv, gate.run(&argv)?)?;
        Self::parse(&String::from_utf8_lossy(&output.stdout))
    }

    /// Parses `ps` output: three numbers, then the command, which may hold spaces.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let processes = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                parse_row(line).ok_or_else(|| Error::Unreadable {
                    argv: Self::argv(),
                    line: line.to_owned(),
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(ProcessTable { processes })
    }

    /// A process whose executable is `path` or lies inside it, compared component by
    /// component as written. Bare names match nothing.
    pub fn executing_from(&self, path: &Path) -> Option<&Process> {
        self.processes
            .iter()
            .find(|process| process.comm.is_absolute() && process.comm.starts_with(path))
    }

    /// The parent of `pid`, its parent, and so on up the chain, nearest first. A chain
    /// that loops ends where it would repeat.
    pub fn ancestors(&self, pid: u32) -> Vec<&Process> {
        let by_pid: HashMap<u32, &Process> = self.processes.iter().map(|p| (p.pid, p)).collect();

        let mut chain = Vec::new();
        let mut seen = HashSet::from([pid]);
        let mut current = by_pid.get(&pid).map(|process| process.ppid);
        while let Some(ppid) = current
            && seen.insert(ppid)
            && let Some(parent) = by_pid.get(&ppid)
        {
            chain.push(*parent);
            current = Some(parent.ppid);
        }
        chain
    }
}

fn parse_row(line: &str) -> Option<Process> {
    let mut rest = line.trim_start();
    let mut field = || {
        let end = rest.find(char::is_whitespace)?;
        let text = &rest[..end];
        rest = rest[end..].trim_start();
        Some(text)
    };
    let pid = field()?.parse().ok()?;
    let ppid = field()?.parse().ok()?;
    let uid = field()?.parse().ok()?;

    match rest.is_empty() {
        true => None,
        false => Some(Process {
            pid,
            ppid,
            uid,
            comm: PathBuf::from(rest),
        }),
    }
}

/// The effective user id rosie runs as, from `id -u`.
pub fn effective_uid<B: Backend>(gate: &Gate<B>) -> Result<u32, Error> {
    let argv = Argv::new("/usr/bin/id").arg("-u");
    let output = succeeded(&argv, gate.run(&argv)?)?;

    let text = String::from_utf8_lossy(&output.stdout);
    text.trim().parse().map_err(|_| Error::Unreadable {
        argv,
        line: text.into_owned(),
    })
}

fn succeeded(argv: &Argv, output: CommandOutput) -> Result<CommandOutput, Error> {
    match output.exit.success() {
        true => Ok(output),
        false => Err(Error::Failed {
            argv: argv.clone(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::fake::FakeBackend;
    use crate::fs::{Bounds, Exit};

    const PS: &str = "\
    1     0     0 /sbin/launchd
  310     1   501 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome
  400   310   501 node
  500     1     0 /usr/bin/sudo
  501   500     0 /Users/me/.cargo/bin/rosie
";

    fn gate(fake: &FakeBackend) -> Gate<&FakeBackend> {
        let bounds = Bounds {
            roots: vec![],
            config_dir: PathBuf::from("/Users/me/.config/rosie"),
            data_dir: PathBuf::from("/Users/me/.local/share/rosie"),
            user_uid: 501,
        };
        Gate::new(fake, bounds).expect("absolute bounds")
    }

    fn output(stdout: &str) -> CommandOutput {
        CommandOutput {
            exit: Exit::Code(0),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    #[test]
    fn parses_commands_holding_spaces() {
        let table = ProcessTable::parse(PS).expect("valid ps output");

        let chrome = table
            .executing_from(Path::new("/Applications/Google Chrome.app"))
            .expect("chrome runs from its bundle");

        assert_eq!(chrome.pid, 310);
        assert_eq!(chrome.uid, 501);
    }

    #[test]
    fn matches_executables_by_component_not_by_prefix() {
        let table = ProcessTable::parse(PS).expect("valid ps output");

        let exact = table.executing_from(Path::new("/usr/bin/sudo"));
        let sibling = table.executing_from(Path::new("/Applications/Google"));
        let bare = table.executing_from(Path::new("node"));

        assert_eq!(exact.map(|p| p.pid), Some(500));
        assert_eq!(sibling, None);
        assert_eq!(bare, None);
    }

    #[test]
    fn walks_the_parent_chain_nearest_first() {
        let table = ProcessTable::parse(PS).expect("valid ps output");

        let pids: Vec<u32> = table.ancestors(501).iter().map(|p| p.pid).collect();

        assert_eq!(pids, vec![500, 1]);
    }

    #[test]
    fn stops_a_looping_parent_chain() {
        let table = ProcessTable::parse("  7  8  0 a\n  8  7  0 b\n").expect("valid");

        let pids: Vec<u32> = table.ancestors(7).iter().map(|p| p.pid).collect();

        assert_eq!(pids, vec![8]);
    }

    #[test]
    fn parses_a_negative_uid_from_a_nobody_owned_process() {
        let table = ProcessTable::parse("51253     1    -2 /usr/libexec/dhcp6d\n")
            .expect("ps prints negative uids for `nobody`");

        let process = table
            .executing_from(Path::new("/usr/libexec/dhcp6d"))
            .expect("dhcp6d is in the table");
        assert_eq!(process.uid, -2);
    }

    #[test]
    fn refuses_an_unreadable_row() {
        let parsed = ProcessTable::parse("  12  1  x /bin/zsh\n");

        assert!(matches!(parsed, Err(Error::Unreadable { .. })));
    }

    #[test]
    fn queries_ps_through_the_gate() {
        let fake = FakeBackend::new();
        fake.respond(ProcessTable::argv(), output(PS));

        let table = ProcessTable::query(&gate(&fake)).expect("canned ps");

        assert_eq!(table.ancestors(400).len(), 2);
    }

    #[test]
    fn reads_the_effective_uid() {
        let fake = FakeBackend::new();
        fake.respond(Argv::new("/usr/bin/id").arg("-u"), output("0\n"));

        let uid = effective_uid(&gate(&fake)).expect("canned id");

        assert_eq!(uid, 0);
    }
}
