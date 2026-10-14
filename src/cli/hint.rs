//! What a user can do about a failure (`docs/spec/cli.md#errors`): a rosie command to
//! run, a flag to run the same command again with, or a step outside rosie. Commands and
//! flags are data built from rosie's own types, never text, so a hint cannot name a
//! command, key, or flag that does not exist.

use std::fmt;
use std::path::PathBuf;

use super::args::flag;
use crate::config::WalkKey;
use crate::fs::EnterLinks;
use crate::output::Palette;
use crate::scan::WalkSetting;
use crate::shell::quote_word;

/// One way out of a failure: what it does, then the steps that do it. A hint with no
/// steps is a step outside rosie, which its label names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub label: String,
    pub steps: Vec<Step>,
}

impl Hint {
    pub fn run(label: impl Into<String>, commands: impl IntoIterator<Item = Command>) -> Hint {
        Hint {
            label: label.into(),
            steps: commands.into_iter().map(Step::Run).collect(),
        }
    }

    pub fn flag(label: impl Into<String>, flag: Flag) -> Hint {
        Hint {
            label: label.into(),
            steps: vec![Step::Flag(flag)],
        }
    }

    /// A step the user takes outside rosie.
    pub fn by_hand(label: impl Into<String>) -> Hint {
        Hint {
            label: label.into(),
            steps: Vec::new(),
        }
    }
}

impl Hint {
    /// The hint on one line, for a line that already says what failed: `to allow it:
    /// rosie roots add ~/x`, or flags run together: `to enter it: --enter-mounts
    /// --enter-bundles`.
    pub fn compact(&self) -> String {
        let steps: Vec<String> = self.steps.iter().map(Step::to_string).collect();
        match steps.is_empty() {
            true => self.label.clone(),
            false => format!("{}: {}", self.label, steps.join(" ")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Run(Command),
    /// Run the same command again with this flag.
    Flag(Flag),
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::Run(command) => write!(f, "{command}"),
            Step::Flag(flag) => write!(f, "{flag}"),
        }
    }
}

/// A rosie command line a hint suggests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Roots,
    RootsAdd(PathBuf),
    /// `rosie roots add <path>`, for the user to fill in.
    RootsAddAny,
    RootsRemove(PathBuf),
    /// `rosie roots remove` with no path: the roots picker.
    RootsRemovePick,
    Config,
    ConfigSetLinks(EnterLinks),
    Rules,
    RulesPull,
    RulesRemove(String),
    ScanTree(PathBuf),
}

impl Command {
    /// The arguments after `rosie`, unquoted; a placeholder reads `<path>`.
    #[cfg(test)]
    pub fn args(&self) -> Vec<String> {
        self.words().into_iter().map(Word::unquoted).collect()
    }

    fn words(&self) -> Vec<Word> {
        let names: &[&str] = match self {
            Command::Roots => &["roots"],
            Command::RootsAdd(_) | Command::RootsAddAny => &["roots", "add"],
            Command::RootsRemove(_) | Command::RootsRemovePick => &["roots", "remove"],
            Command::Config => &["config"],
            Command::ConfigSetLinks(_) => &["config", "set", WalkKey::EnterLinks.name()],
            Command::Rules => &["rules"],
            Command::RulesPull => &["rules", "pull"],
            Command::RulesRemove(_) => &["rules", "remove"],
            Command::ScanTree(_) => &["scan", "tree"],
        };
        let operand = match self {
            Command::RootsAdd(path) | Command::RootsRemove(path) | Command::ScanTree(path) => {
                Some(Word::Given(path.to_string_lossy().into_owned()))
            }
            Command::RootsAddAny => Some(Word::Placeholder("path")),
            Command::ConfigSetLinks(level) => Some(Word::Given(level.label().to_owned())),
            Command::RulesRemove(pack) => Some(Word::Given(pack.clone())),
            Command::Roots
            | Command::RootsRemovePick
            | Command::Config
            | Command::Rules
            | Command::RulesPull => None,
        };
        names
            .iter()
            .map(|name| Word::Given((*name).to_owned()))
            .chain(operand)
            .collect()
    }
}

/// `rosie` and its arguments: each given word one the shell passes through as it is, a
/// placeholder as `<name>`.
impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let words: Vec<String> = self.words().iter().map(Word::shell).collect();
        write!(f, "rosie {}", words.join(" "))
    }
}

/// One argument of a suggested command.
enum Word {
    Given(String),
    /// A value the user supplies.
    Placeholder(&'static str),
}

impl Word {
    #[cfg(test)]
    fn unquoted(self) -> String {
        match self {
            Word::Given(text) => text,
            Word::Placeholder(name) => format!("<{name}>"),
        }
    }

    fn shell(&self) -> String {
        match self {
            Word::Given(text) => shell_word(text),
            Word::Placeholder(name) => format!("<{name}>"),
        }
    }
}

/// A flag that changes what the same command does when run again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    Aggressive,
    Enter(WalkSetting),
}

impl fmt::Display for Flag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Flag::Aggressive => write!(f, "--aggressive"),
            Flag::Enter(setting) => write!(f, "{}", flag(*setting)),
        }
    }
}

/// A word the shell gives rosie as `text`. A path under `~/` keeps its `~/` outside the
/// quotes, so the shell expands it as it does when the user types it.
fn shell_word(text: &str) -> String {
    match text.strip_prefix("~/") {
        Some(rest) => format!("~/{}", quote_word(rest)),
        None => quote_word(text),
    }
}

/// The lines a failure's hints print as, under its problem: each label, the second and
/// later after `or`, then its steps indented below it in the command color.
pub fn hint_lines(palette: Palette, hints: &[Hint]) -> Vec<String> {
    hints
        .iter()
        .enumerate()
        .flat_map(|(index, hint)| {
            let label = match index {
                0 => hint.label.clone(),
                _ => format!("or {}", hint.label),
            };
            let label = match hint.steps.is_empty() {
                true => palette.hint_label(&label),
                false => palette.hint_label(&format!("{label}:")),
            };
            let steps = hint
                .steps
                .iter()
                .map(move |step| format!("  {}", palette.hint_command(&step.to_string())));
            std::iter::once(label).chain(steps)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::super::args::Cli;
    use super::*;

    fn every_command() -> Vec<Command> {
        let mut commands = vec![
            Command::Roots,
            Command::RootsAdd(PathBuf::from("/Users/me/My Code")),
            Command::RootsAddAny,
            Command::RootsRemove(PathBuf::from("~/.cargo/registry")),
            Command::RootsRemovePick,
            Command::Config,
            Command::Rules,
            Command::RulesPull,
            Command::RulesRemove("extra".to_owned()),
            Command::ScanTree(PathBuf::from("~/src")),
        ];
        commands.extend(EnterLinks::ALL.map(Command::ConfigSetLinks));
        commands
    }

    fn every_flag() -> Vec<Flag> {
        let mut flags = vec![Flag::Aggressive];
        flags.extend(
            [
                WalkSetting::Placeholders,
                WalkSetting::Mounts,
                WalkSetting::Bundles,
            ]
            .map(Flag::Enter),
        );
        flags
    }

    /// `docs/spec/cli.md#errors`: a hint names only commands rosie has.
    #[test]
    fn every_suggested_command_parses_as_a_rosie_command_line() {
        for command in every_command() {
            let argv = std::iter::once("rosie".to_owned()).chain(command.args());

            let parsed = Cli::try_parse_from(argv);

            assert!(parsed.is_ok(), "{command}: {parsed:?}");
        }
    }

    #[test]
    fn every_suggested_flag_parses_on_a_tree_scan() {
        for flag in every_flag() {
            let argv = ["rosie", "scan", "tree"]
                .map(str::to_owned)
                .into_iter()
                .chain([flag.to_string()]);

            let parsed = Cli::try_parse_from(argv);

            assert!(parsed.is_ok(), "{flag}: {parsed:?}");
        }
    }

    #[test]
    fn a_command_quotes_what_the_shell_would_split_and_keeps_a_home_path_expandable() {
        let spaced = Command::RootsAdd(PathBuf::from("/Users/me/My Code"));
        let home = Command::RootsRemove(PathBuf::from("~/.cargo/registry"));
        let odd_home = Command::RootsRemove(PathBuf::from("~/My Code"));

        assert_eq!(spaced.to_string(), "rosie roots add '/Users/me/My Code'");
        assert_eq!(home.to_string(), "rosie roots remove ~/.cargo/registry");
        assert_eq!(odd_home.to_string(), "rosie roots remove ~/'My Code'");
        assert_eq!(Command::RootsAddAny.to_string(), "rosie roots add <path>");
    }

    #[test]
    fn later_fixes_read_as_alternatives_with_their_steps_indented() {
        let hints = [
            Hint::run(
                "allow symlinks in roots",
                [Command::ConfigSetLinks(EnterLinks::Roots)],
            ),
            Hint::by_hand("quit Slack, then run again"),
        ];

        let lines = hint_lines(Palette::plain(), &hints);

        assert_eq!(
            lines,
            [
                "allow symlinks in roots:",
                "  rosie config set walk.enter_links roots",
                "or quit Slack, then run again",
            ]
        );
    }
}
