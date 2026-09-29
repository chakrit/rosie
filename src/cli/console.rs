//! The terminal side of the CLI: output streams, stdin, prompts, and progress. `main`
//! injects [`SystemConsole`]; the sandbox tests inject a scripted console.

use std::fmt::Display;
use std::io::{self, IsTerminal, Read, Stderr, Stdout, Write};

use inquire::{InquireError, MultiSelect, Select, Text};

use crate::fs::Argv;
use crate::run::elevated::Stdin;
use crate::run::{ItemResult, OutputWindow, PlainReporter, Reporter, TerminalReporter};
use crate::scan::{Progress, StderrProgress};

/// Which standard streams are terminals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Terminals {
    pub stdin: bool,
    pub stdout: bool,
    pub stderr: bool,
}

impl Terminals {
    /// Pickers and the `clean` prompt need a person at stdin reading stdout
    /// (`docs/spec/cli.md#pickers`).
    pub fn interactive(self) -> bool {
        self.stdin && self.stdout
    }
}

/// Why a prompt returned no answer.
#[derive(Debug)]
pub enum PromptError {
    /// The user cancelled it.
    Cancelled,
    Io(io::Error),
}

/// Something that ends with output it may still have to flush.
pub trait Finish {
    fn finish(self) -> io::Result<()>;
}

pub trait Console {
    type Out: Write + Send;
    type Err: Write;
    type Progress: Progress + Finish;
    type Reporter: Reporter + Send + Finish;

    fn terminals(&self) -> Terminals;
    fn stdout(&mut self) -> &mut Self::Out;
    fn stderr(&mut self) -> &mut Self::Err;

    /// Reads stdin to its end, whatever it is: `rosie run -`.
    fn read_stdin(&mut self) -> io::Result<Vec<u8>>;

    /// Reads stdin to its end only when it is a pipe: the elevated child.
    fn capture_stdin(&mut self) -> io::Result<Stdin>;

    /// Asks one line of text.
    fn ask(&mut self, prompt: &str) -> Result<String, PromptError>;

    /// Lets the user pick one of `options`; returns its position.
    fn pick(&mut self, prompt: &str, options: &[String]) -> Result<usize, PromptError>;

    /// Shows `items`, all ticked; returns the items the user left ticked.
    fn checklist<T: Display>(&mut self, prompt: &str, items: Vec<T>)
    -> Result<Vec<T>, PromptError>;

    /// Scan progress on stderr.
    fn progress(&self) -> Self::Progress;

    /// Run progress on stderr.
    fn reporter(&self) -> Self::Reporter;
}

/// The process's own standard streams, with `inquire` prompts.
pub struct SystemConsole {
    terminals: Terminals,
    stdout: Stdout,
    stderr: Stderr,
}

impl SystemConsole {
    pub fn new() -> Self {
        let terminals = Terminals {
            stdin: io::stdin().is_terminal(),
            stdout: io::stdout().is_terminal(),
            stderr: io::stderr().is_terminal(),
        };
        SystemConsole {
            terminals,
            stdout: io::stdout(),
            stderr: io::stderr(),
        }
    }
}

impl Default for SystemConsole {
    fn default() -> Self {
        SystemConsole::new()
    }
}

impl Console for SystemConsole {
    type Out = Stdout;
    type Err = Stderr;
    type Progress = StderrProgress;
    type Reporter = StderrReporter;

    fn terminals(&self) -> Terminals {
        self.terminals
    }

    fn stdout(&mut self) -> &mut Stdout {
        &mut self.stdout
    }

    fn stderr(&mut self) -> &mut Stderr {
        &mut self.stderr
    }

    fn read_stdin(&mut self) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        io::stdin().read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn capture_stdin(&mut self) -> io::Result<Stdin> {
        Stdin::capture(io::stdin())
    }

    fn ask(&mut self, prompt: &str) -> Result<String, PromptError> {
        Text::new(prompt).prompt().map_err(prompt_error)
    }

    fn pick(&mut self, prompt: &str, options: &[String]) -> Result<usize, PromptError> {
        Select::new(prompt, options.to_vec())
            .raw_prompt()
            .map(|chosen| chosen.index)
            .map_err(prompt_error)
    }

    fn checklist<T: Display>(
        &mut self,
        prompt: &str,
        items: Vec<T>,
    ) -> Result<Vec<T>, PromptError> {
        MultiSelect::new(prompt, items)
            .with_all_selected_by_default()
            .prompt()
            .map_err(prompt_error)
    }

    fn progress(&self) -> StderrProgress {
        StderrProgress::new()
    }

    fn reporter(&self) -> StderrReporter {
        match self.terminals.stderr {
            true => StderrReporter::Terminal(TerminalReporter::default()),
            false => StderrReporter::Plain(PlainReporter::new(io::stderr())),
        }
    }
}

fn prompt_error(error: InquireError) -> PromptError {
    match error {
        InquireError::OperationCanceled | InquireError::OperationInterrupted => {
            PromptError::Cancelled
        }
        InquireError::IO(error) => PromptError::Io(error),
        other => PromptError::Io(io::Error::other(other.to_string())),
    }
}

/// A spinner with live command output on a terminal stderr, plain result lines
/// otherwise (`docs/spec/plan.md#run`).
pub enum StderrReporter {
    Terminal(TerminalReporter),
    Plain(PlainReporter<Stderr>),
}

impl Reporter for StderrReporter {
    fn command_started(&mut self, argv: &Argv) {
        match self {
            StderrReporter::Terminal(reporter) => reporter.command_started(argv),
            StderrReporter::Plain(reporter) => reporter.command_started(argv),
        }
    }

    fn command_output(&mut self, argv: &Argv, window: &OutputWindow) {
        match self {
            StderrReporter::Terminal(reporter) => reporter.command_output(argv, window),
            StderrReporter::Plain(reporter) => reporter.command_output(argv, window),
        }
    }

    fn item_finished(&mut self, result: &ItemResult) {
        match self {
            StderrReporter::Terminal(reporter) => reporter.item_finished(result),
            StderrReporter::Plain(reporter) => reporter.item_finished(result),
        }
    }

    fn elevating(&mut self, items: usize) {
        match self {
            StderrReporter::Terminal(reporter) => reporter.elevating(items),
            StderrReporter::Plain(reporter) => reporter.elevating(items),
        }
    }
}

impl Finish for StderrReporter {
    fn finish(self) -> io::Result<()> {
        match self {
            StderrReporter::Terminal(reporter) => reporter.finish(),
            StderrReporter::Plain(reporter) => reporter.finish(),
        }
    }
}

impl<W: Write> Finish for PlainReporter<W> {
    fn finish(self) -> io::Result<()> {
        PlainReporter::finish(self)
    }
}

impl Finish for StderrProgress {
    fn finish(self) -> io::Result<()> {
        StderrProgress::finish(&self);
        Ok(())
    }
}

impl Finish for crate::scan::NoProgress {
    fn finish(self) -> io::Result<()> {
        Ok(())
    }
}
