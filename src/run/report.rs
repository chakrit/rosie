//! What a run shows on stderr while it goes (`docs/spec/plan.md#run`).
//!
//! While a command runs, a terminal shows a spinner with the command and a live
//! window of its last 3 output lines; when it finishes, those lines stay as its result.
//! When stderr is not a terminal, only the results are printed, each command with its
//! final 3 lines.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

use super::ItemResult;
use crate::fs::Argv;

/// How many output lines of a command are shown.
const WINDOW: usize = 3;

const SPINNER_TICK: Duration = Duration::from_millis(100);

/// Receives a run's progress. Deletes are reported when their part of the run has
/// finished; commands as they run.
pub trait Reporter {
    fn command_started(&mut self, argv: &Argv);

    /// A new output line arrived; `window` holds the last lines, the newest last.
    fn command_output(&mut self, argv: &Argv, window: &OutputWindow);

    fn item_finished(&mut self, result: &ItemResult);

    /// The run is about to ask sudo to run `items` items in an elevated child.
    fn elevating(&mut self, items: usize);
}

/// The last 3 lines of a command's output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputWindow {
    lines: VecDeque<String>,
}

impl OutputWindow {
    pub fn push(&mut self, line: &str) {
        if self.lines.len() == WINDOW {
            self.lines.pop_front();
        }
        self.lines.push_back(line.to_owned());
    }

    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }

    pub fn into_lines(self) -> Vec<String> {
        self.lines.into()
    }
}

/// Prints each result as a line, commands with their final 3 lines, and nothing while
/// a command runs: for a stderr that is not a terminal.
///
/// A failed write stops further output; [`PlainReporter::finish`] returns it.
pub struct PlainReporter<W: Write> {
    out: W,
    error: Option<io::Error>,
}

impl<W: Write> PlainReporter<W> {
    pub fn new(out: W) -> Self {
        PlainReporter { out, error: None }
    }

    /// The first write error, if any write failed.
    pub fn finish(self) -> io::Result<()> {
        match self.error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn line(&mut self, text: &str) {
        if self.error.is_some() {
            return;
        }
        if let Err(error) = writeln!(self.out, "{text}") {
            self.error = Some(error);
        }
    }
}

impl<W: Write> Reporter for PlainReporter<W> {
    fn command_started(&mut self, _argv: &Argv) {}

    fn command_output(&mut self, _argv: &Argv, _window: &OutputWindow) {}

    fn item_finished(&mut self, result: &ItemResult) {
        self.line(&result.to_string());
    }

    fn elevating(&mut self, items: usize) {
        self.line(&elevating_line(items));
    }
}

/// A spinner with the command and its live 3-line window while a command runs, then
/// the results as [`PlainReporter`] prints them: for a stderr that is a terminal.
pub struct TerminalReporter {
    results: PlainReporter<io::Stderr>,
    spinner: Option<ProgressBar>,
}

impl Default for TerminalReporter {
    fn default() -> Self {
        TerminalReporter {
            results: PlainReporter::new(io::stderr()),
            spinner: None,
        }
    }
}

impl TerminalReporter {
    pub fn finish(self) -> io::Result<()> {
        self.results.finish()
    }
}

impl Reporter for TerminalReporter {
    fn command_started(&mut self, argv: &Argv) {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::with_template("{spinner} {msg}").unwrap_or_else(|error| {
                unreachable!("the spinner template is fixed and valid: {error}")
            }),
        );
        spinner.set_message(spinner_message(argv, &OutputWindow::default()));
        spinner.enable_steady_tick(SPINNER_TICK);
        self.spinner = Some(spinner);
    }

    fn command_output(&mut self, argv: &Argv, window: &OutputWindow) {
        if let Some(spinner) = &self.spinner {
            spinner.set_message(spinner_message(argv, window));
        }
    }

    fn item_finished(&mut self, result: &ItemResult) {
        if let Some(spinner) = self.spinner.take() {
            spinner.finish_and_clear();
        }
        self.results.item_finished(result);
    }

    fn elevating(&mut self, items: usize) {
        self.results.elevating(items);
    }
}

/// The command, then its window, each line indented under it.
fn spinner_message(argv: &Argv, window: &OutputWindow) -> String {
    let mut message = argv.to_string();
    for line in window.lines() {
        message.push_str("\n  | ");
        message.push_str(line);
    }
    message
}

fn elevating_line(items: usize) -> String {
    let noun = match items {
        1 => "item",
        _ => "items",
    };
    format!("running {items} {noun} with sudo")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::plan::{ItemSize, Outcome, Size, SkipReason};
    use crate::run::Subject;

    fn window_of(lines: &[&str]) -> OutputWindow {
        let mut window = OutputWindow::default();
        for line in lines {
            window.push(line);
        }
        window
    }

    #[test]
    fn keeps_the_last_three_lines() {
        let window = window_of(&["1", "2", "3", "4", "5"]);

        assert_eq!(window.into_lines(), vec!["3", "4", "5"]);
    }

    #[test]
    fn the_spinner_shows_the_command_over_its_window() {
        let argv = Argv::new("docker").arg("system").arg("prune");
        let window = window_of(&["a", "b", "c", "d"]);

        assert_eq!(
            spinner_message(&argv, &window),
            "docker system prune\n  | b\n  | c\n  | d"
        );
    }

    #[test]
    fn plain_output_shows_only_results_with_the_final_lines() {
        let argv = Argv::new("gradle").arg("--stop");
        let mut reporter = PlainReporter::new(Vec::new());

        reporter.command_started(&argv);
        reporter.command_output(&argv, &window_of(&["starting"]));
        reporter.item_finished(&ItemResult {
            subject: Subject::Command {
                argv: argv.clone(),
                output: vec!["b".into(), "c".into(), "d".into()],
            },
            size: ItemSize::Unknown,
            outcome: Outcome::Done,
            reason: String::new(),
        });
        reporter.item_finished(&ItemResult {
            subject: Subject::Path(PathBuf::from("/w/node_modules")),
            size: ItemSize::Known(Size::bytes(1_500_000)),
            outcome: Outcome::Skipped(SkipReason::Stale),
            reason: "no longer exists".into(),
        });
        reporter.elevating(2);

        let PlainReporter { out, error } = reporter;
        assert!(error.is_none());
        assert_eq!(
            String::from_utf8(out).expect("utf-8"),
            "ran gradle --stop\n  | b\n  | c\n  | d\n\
             skipped (stale) /w/node_modules (1.5 MB): no longer exists\n\
             running 2 items with sudo\n"
        );
    }
}
