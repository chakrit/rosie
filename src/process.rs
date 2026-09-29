//! Facts about running processes and the current user, read from system CLIs through
//! the gate's command method (`docs/spec/safety.md#running-processes`,
//! `docs/decisions/2026-09-29-elevation-without-ffi.md`).
//!
//! Shared by the scanner, app mode, and the runner: all of them refuse items a process
//! executes from, and the elevated entry walks the parent chain to find `sudo`.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;

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
    pub uid: u32,
    /// The executable as `ps` prints it, byte for byte: a full path, or a bare name.
    pub comm: PathBuf,
}

impl Process {
    /// Whether this process executes from `path` or from inside it, compared component
    /// by component as written. A bare name never matches.
    pub fn executes_from(&self, path: &Path) -> bool {
        self.comm.is_absolute() && self.comm.starts_with(path)
    }
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
        Self::parse(&output.stdout)
    }

    /// Parses `ps` output: three numbers, then the command, which is the rest of the
    /// line and may hold spaces. The command keeps its exact bytes, so a path that is
    /// not UTF-8 still matches the item it runs from.
    pub fn parse(text: &[u8]) -> Result<Self, Error> {
        let processes = text
            .split(|&byte| byte == b'\n')
            .filter(|line| !line.trim_ascii().is_empty())
            .map(|line| {
                parse_row(line).ok_or_else(|| Error::Unreadable {
                    argv: Self::argv(),
                    line: String::from_utf8_lossy(line).into_owned(),
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(ProcessTable { processes })
    }

    /// A process whose executable is `path` or lies inside it.
    pub fn executing_from(&self, path: &Path) -> Option<&Process> {
        self.processes
            .iter()
            .find(|process| process.executes_from(path))
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

fn parse_row(line: &[u8]) -> Option<Process> {
    let (pid, rest) = next_field(line)?;
    let (ppid, rest) = next_field(rest)?;
    let (uid, comm) = next_field(rest)?;
    if comm.is_empty() {
        return None;
    }

    // `ps` prints `uid_t` reinterpreted signed, so a uid outside `int32` range, such as
    // `nobody` (4294967294), shows as a negative number.
    let uid: i32 = number(uid)?;
    Some(Process {
        pid: number(pid)?,
        ppid: number(ppid)?,
        uid: uid.cast_unsigned(),
        comm: PathBuf::from(OsStr::from_bytes(comm)),
    })
}

/// The first whitespace-separated field of `text` and what follows it, with the
/// whitespace between them removed.
fn next_field(text: &[u8]) -> Option<(&[u8], &[u8])> {
    let text = text.trim_ascii_start();
    let end = text.iter().position(u8::is_ascii_whitespace)?;
    Some((&text[..end], text[end..].trim_ascii_start()))
}

fn number<T: FromStr>(field: &[u8]) -> Option<T> {
    std::str::from_utf8(field).ok()?.parse().ok()
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
mod tests;
