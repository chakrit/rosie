//! The hidden `__elevated` entry: the root child a run launches through sudo for its
//! `sudo` items (`docs/spec/safety.md#internal-elevated-entry`,
//! `docs/decisions/2026-09-29-elevation-without-ffi.md`).
//!
//! It acts only when all four gates pass:
//!
//! 1. stdin is a pipe;
//! 2. stdin starts with a header of this format version whose nonce matches the
//!    argument;
//! 3. it runs as root, by `id -u`;
//! 4. an ancestor process is `sudo`, by the `ps` parent chain.
//!
//! It then loads the invoking user's config and roots from the header's home, never
//! `$HOME`, and runs the piped items through the same gate and runner as any run.

use std::fs::File;
use std::io::{self, Read as _, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::FileTypeExt as _;

use thiserror::Error;

use super::header::{self, Header, Nonce};
use super::{ItemResult, OutputWindow, Reporter, Runner, outcomes};
use crate::config;
use crate::fs::{self, Argv, Backend, Gate, Home, ROOT_UID};
use crate::plan::{self, Plan, RunStats};
use crate::process::{self, Process, ProcessTable};

/// The hidden subcommand name.
pub const SUBCOMMAND: &str = "__elevated";

/// What the child's stdin was when it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stdin {
    /// A pipe, read to its end.
    Pipe(Vec<u8>),
    /// A terminal, a file, or anything else; never read.
    NotPipe,
}

impl Stdin {
    /// Reads `source` to its end when it is a pipe. Anything else is left unread, so a
    /// terminal never blocks the refusal.
    pub fn capture(source: impl AsFd) -> io::Result<Stdin> {
        let mut file = File::from(source.as_fd().try_clone_to_owned()?);
        if !file.metadata()?.file_type().is_fifo() {
            return Ok(Stdin::NotPipe);
        }

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Stdin::Pipe(bytes))
    }
}

/// How the child was invoked, gathered by `main`.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// The argument after `__elevated`.
    pub nonce: String,
    pub stdin: Stdin,
    /// This process's id, where the `ps` parent chain starts.
    pub pid: u32,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("refused: stdin is not a pipe; `{SUBCOMMAND}` is only for rosie itself")]
    NotAPipe,

    #[error("refused: {0}")]
    Header(#[from] header::Error),

    #[error("refused: the nonce does not match the stdin header")]
    NonceMismatch,

    #[error("refused: not running as root (uid {uid})")]
    NotRoot { uid: u32 },

    #[error("refused: no ancestor process is sudo")]
    NoSudoAncestor,

    #[error(
        "refused: tool commands never run as root; only deletes, their bootouts, and receipts are elevated"
    )]
    ToolElevated,

    #[error(transparent)]
    Process(#[from] process::Error),

    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error(transparent)]
    Config(#[from] config::Error),

    #[error("the elevated items are not a valid plan: {0}")]
    Plan(#[from] plan::Error),

    #[error(transparent)]
    Outcomes(#[from] super::outcomes::Error),
}

/// Runs the piped items as root once every gate passes, writes each item's outcome to
/// `out` for the run that launched it as the item finishes, and returns the stats.
pub fn run_elevated<B, R, W>(
    backend: &B,
    invocation: Invocation,
    reporter: &mut R,
    out: W,
) -> Result<RunStats, Error>
where
    B: Backend + Sync,
    R: Reporter,
    W: Write,
{
    let (header, body) = admit(backend, &invocation)?;
    let plan = Plan::parse(body)?;
    if !plan.tools().is_empty() {
        return Err(Error::ToolElevated);
    }
    let gate = user_gate(backend, &header.home)?;

    let mut reporting = ReportingToRun {
        reporter,
        writer: outcomes::Writer::begin(out)?,
        failure: None,
    };
    let results = Runner::as_root(gate).execute(&plan, &mut reporting);
    if let Some(error) = reporting.failure {
        return Err(error.into());
    }

    let mut stats = RunStats::default();
    for result in results {
        stats.record(result.size, result.outcome);
    }
    Ok(stats)
}

/// Hands every event to the child's own reporter, and writes each finished item's
/// outcome for the run that launched the child at once, so a crash later in the run
/// cannot lose it.
struct ReportingToRun<'r, R, W: Write> {
    reporter: &'r mut R,
    writer: outcomes::Writer<W>,
    /// The first write that failed; the run learns of it when the items are done.
    failure: Option<outcomes::Error>,
}

impl<R: Reporter, W: Write> Reporter for ReportingToRun<'_, R, W> {
    fn command_started(&mut self, argv: &Argv) {
        self.reporter.command_started(argv);
    }

    fn command_output(&mut self, argv: &Argv, window: &OutputWindow) {
        self.reporter.command_output(argv, window);
    }

    fn item_finished(&mut self, result: &ItemResult) {
        self.reporter.item_finished(result);
        if self.failure.is_none()
            && let Err(error) = self.writer.record(result)
        {
            self.failure = Some(error);
        }
    }

    fn elevating(&mut self, items: usize) {
        self.reporter.elevating(items);
    }
}

/// The four gates. Nothing acts before all of them pass.
fn admit<'a, B: Backend>(
    backend: &B,
    invocation: &'a Invocation,
) -> Result<(Header, &'a str), Error> {
    let Stdin::Pipe(bytes) = &invocation.stdin else {
        return Err(Error::NotAPipe);
    };
    let (header, body) = Header::decode(bytes)?;
    if Nonce::parse(&invocation.nonce)? != header.nonce {
        return Err(Error::NonceMismatch);
    }

    let gate = Gate::new(backend, header.home.bounds(Vec::new(), ROOT_UID))?;
    let uid = process::effective_uid(&gate)?;
    if uid != ROOT_UID {
        return Err(Error::NotRoot { uid });
    }
    let processes = ProcessTable::query(&gate)?;
    let under_sudo = processes
        .ancestors(invocation.pid)
        .iter()
        .any(|process| is_root_sudo(process));
    if !under_sudo {
        return Err(Error::NoSudoAncestor);
    }

    Ok((header, body))
}

/// A `sudo` process running as root. Any user can start a process whose `ps` name is
/// `sudo`; only root can make it run as root.
fn is_root_sudo(process: &Process) -> bool {
    process.uid == ROOT_UID && process.comm.file_name() == Some("sudo".as_ref())
}

/// A gate acting for the user whose home this is, bounded by the roots in their
/// config. The user is the owner of the home folder. The child deletes through it only
/// what `Gate::admit_delete_as_root` admits, which changes no permissions.
fn user_gate<'b, B: Backend>(backend: &'b B, home: &Home) -> Result<Gate<&'b B>, Error> {
    let bootstrap = Gate::new(backend, home.bounds(Vec::new(), ROOT_UID))?;
    let user_uid = bootstrap.lstat(home)?.uid;
    let file = config::load(&bootstrap, &home.config_dir(), home)?;

    let roots = file.config().roots.clone();
    Ok(Gate::new(backend, home.bounds(roots, user_uid))?)
}

#[cfg(test)]
mod tests;
