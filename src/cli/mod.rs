//! The command line: parses argv and wires the modules into each command
//! (`docs/spec/cli.md`).
//!
//! [`App`] runs in-process over an injected backend, network, and console, with every
//! environment-derived input in [`Env`]; `main` builds the real ones and reads `$HOME`
//! once. Nothing below `main` reads the environment.

mod args;
mod console;
mod error;
mod findings;
mod manage;
mod scan;
mod session;

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;

use clap::Parser;
use clap::error::ErrorKind;

use crate::fs::{Backend, Home};
use crate::packs::Network;
use crate::plan::RunStats;
use crate::run::RootError;
use crate::run::elevated::{self, Invocation};

use args::{Cli, Command, UserCommand};
use session::Session;

pub use console::{Console, Finish, PromptError, StderrReporter, SystemConsole, Terminals};
pub use error::Error;

/// Everything rosie takes from its environment, gathered by `main`.
#[derive(Debug, Clone)]
pub struct Env {
    pub home: Home,
    /// The current folder: absolute, and the physical path the kernel reports.
    pub cwd: PathBuf,
    /// This rosie executable, which elevation launches through sudo.
    pub exe: PathBuf,
    pub pid: u32,
    pub now: SystemTime,
}

/// How rosie exits (`docs/spec/cli.md#exit-status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    Success,
    /// An item failed or the command errored.
    Failed,
    /// The command line was wrong.
    Usage,
}

impl ExitStatus {
    pub fn code(self) -> u8 {
        match self {
            ExitStatus::Success => 0,
            ExitStatus::Failed => 1,
            ExitStatus::Usage => 2,
        }
    }

    fn failed_if(failed: bool) -> ExitStatus {
        match failed {
            true => ExitStatus::Failed,
            false => ExitStatus::Success,
        }
    }
}

pub struct App<B: Backend, N: Network, C: Console> {
    backend: B,
    network: N,
    console: C,
}

impl<B: Backend + Sync, N: Network, C: Console> App<B, N, C> {
    pub fn new(backend: B, network: N, console: C) -> Self {
        App {
            backend,
            network,
            console,
        }
    }

    /// Runs one command line; `argv` starts with the program name. `env` is called only
    /// once argument parsing finds a command that needs it, so `--help` and `--version`
    /// never require an environment to be gathered at all.
    pub fn run(
        &mut self,
        argv: impl IntoIterator<Item = OsString>,
        env: impl FnOnce() -> Result<Env, String>,
    ) -> ExitStatus {
        let argv: Vec<OsString> = argv.into_iter().collect();
        let cli = match Cli::try_parse_from(&argv) {
            Ok(cli) => cli,
            Err(error) => return self.show_parse_error(&error),
        };

        let env = match env() {
            Ok(env) => env,
            Err(reason) => return self.show_env_error(&reason),
        };

        let outcome = match cli.command {
            Some(Command::Elevated { nonce }) => self.elevated(&env, nonce),
            Some(Command::User(command)) => self.command(&env, command, typed_args(&argv)),
            None => self.command(&env, UserCommand::default(), typed_args(&argv)),
        };
        match outcome {
            Ok(status) => status,
            Err(error) => self.show_error(&error),
        }
    }

    fn command(
        &mut self,
        env: &Env,
        command: UserCommand,
        typed: Vec<String>,
    ) -> Result<ExitStatus, Error> {
        let mut session =
            Session::open(&self.backend, &self.network, &mut self.console, env, typed)?;
        session.dispatch(command)
    }

    /// The hidden entry the run launches through sudo; it passes its own four gates
    /// and never reaches the `sudo rosie` refusal.
    fn elevated(&mut self, env: &Env, nonce: String) -> Result<ExitStatus, Error> {
        let invocation = Invocation {
            nonce,
            stdin: self.console.capture_stdin()?,
            pid: env.pid,
        };

        let mut reporter = self.console.reporter();
        let stats = elevated::run_elevated(
            &self.backend,
            invocation,
            &mut reporter,
            self.console.stdout(),
        );
        reporter.finish()?;

        let stats: RunStats = stats?;
        Ok(ExitStatus::failed_if(stats.any_failed()))
    }

    // errors

    /// The environment could not be gathered (for example `$HOME` is not set); no
    /// command needing it ever ran.
    fn show_env_error(&mut self, reason: &str) -> ExitStatus {
        match writeln!(self.console.stderr(), "rosie: {reason}") {
            Ok(()) | Err(_) => ExitStatus::Failed,
        }
    }

    fn show_parse_error(&mut self, error: &clap::Error) -> ExitStatus {
        let text = error.to_string();
        let (written, status) = match error.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                (write!(self.console.stdout(), "{text}"), ExitStatus::Success)
            }
            _ => (write!(self.console.stderr(), "{text}"), ExitStatus::Usage),
        };
        match written {
            Ok(()) => status,
            Err(_) => ExitStatus::Failed,
        }
    }

    fn show_error(&mut self, error: &Error) -> ExitStatus {
        let status = match error {
            Error::Usage(_) => ExitStatus::Usage,
            _ => ExitStatus::Failed,
        };
        let written = match error {
            Error::Root(RootError::Refused(refusal)) => {
                writeln!(self.console.stderr(), "{refusal}")
            }
            // A usage message already reads as a full sentence to the user (it names
            // `rosie` or starts with `usage:` itself), so it takes no extra prefix.
            Error::Usage(message) => writeln!(self.console.stderr(), "{message}"),
            _ => writeln!(self.console.stderr(), "rosie: {error}"),
        };
        match written {
            Ok(()) => status,
            Err(_) => ExitStatus::Failed,
        }
    }
}

/// The arguments after the program name, as typed, for the `sudo rosie` refusal to
/// repeat.
fn typed_args(argv: &[OsString]) -> Vec<String> {
    argv.iter()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}
