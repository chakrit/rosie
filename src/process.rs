//! Facts about running processes and the current user, read from system CLIs through
//! the gate's command method (`docs/spec/safety.md#running-processes`,
//! `docs/decisions/2026-09-29-elevation-without-ffi.md`).
//!
//! Shared by the scanner, app mode, and the runner: all of them refuse items a process
//! executes from, and the elevated entry walks the parent chain to find `sudo`. A scanned
//! plan's path entry is built only from an [`IdlePath`], which only
//! [`ProcessTable::admit`] issues, so no scan mode can plan an item without passing it
//! through the table. A plan read from a file carries no such proof; the runner checks
//! its deletes against the table before running them.
//!
//! [`run_tool`] and [`run_tool_text`] are the one way rosie reads a read-only system
//! tool's output, here and in app mode (`plutil`, `pkgutil`).

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use thiserror::Error;

use crate::fs::{self, Argv, Backend, Exit, Gate, SystemArgv, SystemTool};

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessTable {
    processes: Vec<Process>,
}

impl ProcessTable {
    pub fn argv() -> SystemArgv {
        SystemTool::PS
            .argv()
            .arg("-axo")
            .arg("pid=,ppid=,uid=,comm=")
    }

    /// Reads the table with `ps`.
    pub fn query<B: Backend>(gate: &Gate<B>) -> Result<Self, Error> {
        let stdout = run_tool(gate, &Self::argv())?;
        Self::parse(&stdout)
    }

    /// Parses `ps` output: three numbers, then the command, which is the rest of the
    /// line and may hold spaces. The command keeps its exact bytes, so a path that is
    /// not UTF-8 still matches the item it runs from.
    pub(crate) fn parse(text: &[u8]) -> Result<Self, Error> {
        let processes = text
            .split(|&byte| byte == b'\n')
            .filter(|line| !line.trim_ascii().is_empty())
            .map(|line| {
                parse_row(line).ok_or_else(|| Error::Unreadable {
                    argv: Self::argv().into(),
                    line: String::from_utf8_lossy(line).into_owned(),
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(ProcessTable { processes })
    }

    /// A table with no process, which admits every path: only tests build one, so every
    /// production table comes from `ps`.
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        ProcessTable {
            processes: Vec::new(),
        }
    }

    /// A process whose executable is `path` or lies inside it.
    pub fn executing_from(&self, path: &Path) -> Option<&Process> {
        self.processes
            .iter()
            .find(|process| process.executes_from(path))
    }

    /// Admits `path` as an item a plan may hold when no process executes from it, or
    /// names the process that does.
    pub fn admit(&self, path: PathBuf) -> Admission<'_> {
        match self.executing_from(&path) {
            Some(process) => Admission::Running { path, process },
            None => Admission::Idle(IdlePath(path)),
        }
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

/// Whether a path may become a plan item (`docs/spec/safety.md#running-processes`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission<'t> {
    Idle(IdlePath),
    Running { path: PathBuf, process: &'t Process },
}

/// A path no process executed from when the process table was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdlePath(PathBuf);

impl IdlePath {
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_path(self) -> PathBuf {
        self.0
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
    let argv = SystemTool::ID.argv().arg("-u");
    let text = run_tool_text(gate, &argv)?;
    text.trim().parse().map_err(|_| Error::Unreadable {
        argv: argv.into(),
        line: text,
    })
}

/// Runs a read-only system tool through the gate and returns its standard output. A
/// non-zero exit fails with the tool's standard error.
pub fn run_tool<B: Backend>(gate: &Gate<B>, argv: &SystemArgv) -> Result<Vec<u8>, Error> {
    let argv = argv.as_argv();
    let output = gate.run(argv)?;
    match output.exit.success() {
        true => Ok(output.stdout),
        false => Err(Error::Failed {
            argv: argv.clone(),
            exit: output.exit,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        }),
    }
}

/// [`run_tool`] for a tool that prints text; output that is not UTF-8 fails.
pub fn run_tool_text<B: Backend>(gate: &Gate<B>, argv: &SystemArgv) -> Result<String, Error> {
    let stdout = run_tool(gate, argv)?;
    String::from_utf8(stdout).map_err(|_| Error::NotUtf8 {
        argv: argv.as_argv().clone(),
    })
}

#[cfg(test)]
mod tests;
