//! The command line, exactly as `docs/spec/cli.md` lays it out. Flags parse into the
//! plain values clap needs; `super` turns them into typed settings once.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::config::Walk;
use crate::config::WalkSetting;
use crate::fs::EnterLinks;
use crate::plan::AggressiveItems;
use crate::rules::Name;
use crate::run::elevated;
use crate::scan::{Needs, Settings, WalkSkip};

use super::Error;
use crate::hint::{Flag, Hint, Step};

#[derive(Debug, Parser)]
#[command(name = "rosie", version, about = "A macOS cleanup CLI for developers.")]
pub struct Cli {
    /// Without a command, rosie runs `rosie clean tree .`.
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(flatten)]
    User(UserCommand),

    #[command(name = elevated::SUBCOMMAND, hide = true)]
    Elevated { nonce: String },
}

/// Every command a user runs; each passes the `sudo rosie` refusal first.
#[derive(Debug, Subcommand)]
pub enum UserCommand {
    /// Print the plan as TOML on stdout, stats on stderr.
    #[command(visible_alias = "plan")]
    Scan(ScanArgs),

    /// Scan, confirm, and run.
    Clean(CleanArgs),

    /// Say, per path, what a plan would do with it.
    Check(CheckArgs),

    /// Execute a plan file, or `-` to read the plan from stdin.
    Run {
        #[arg(value_parser = parse_plan_source)]
        plan: PlanSource,
    },

    /// Show the rules with their packs, or manage the packs.
    Rules {
        #[command(subcommand)]
        action: Option<RulesAction>,
    },

    /// Show the `roots` allowlist, or change it.
    Roots {
        #[command(subcommand)]
        action: Option<RootsAction>,
    },

    /// Show every config key and its value, or change one.
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
}

impl Default for UserCommand {
    /// Bare `rosie` is `rosie clean tree .`.
    fn default() -> Self {
        UserCommand::Clean(CleanArgs {
            mode: Mode::default(),
            flags: ScanFlags::default(),
        })
    }
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    #[command(subcommand)]
    pub mode: Mode,

    #[command(flatten)]
    pub flags: ScanFlags,

    #[command(flatten)]
    pub format: FormatFlags,
}

/// How `scan` prints the plan (`docs/spec/cli.md#flags`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// The plan listing, for reading.
    List,
    /// The plan file.
    Toml,
    /// A shell script to inspect and run by hand.
    Sh,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::List, Format::Toml, Format::Sh];

    pub fn label(self) -> &'static str {
        match self {
            Format::List => "list",
            Format::Toml => "toml",
            Format::Sh => "sh",
        }
    }
}

/// `--format` and its shortcuts. Clap does not check a group of global flags given after
/// the mode, so [`FormatFlags::or`] refuses more than one.
#[derive(Debug, Args)]
pub struct FormatFlags {
    /// `list`, `toml`, or `sh` (default: a list on a terminal, TOML otherwise).
    #[arg(long, global = true, value_name = "FORMAT", value_parser = parse_format)]
    format: Option<Format>,

    /// Print the plan file: --format=toml.
    #[arg(long, global = true)]
    toml: bool,

    /// Print a shell script to inspect and run yourself: --format=sh.
    #[arg(long, global = true)]
    sh: bool,
}

impl FormatFlags {
    /// The one format asked for, or `by_default` when no format flag was given.
    pub fn or(&self, by_default: Format) -> Result<Format, Error> {
        let toml = self.toml.then_some(Format::Toml);
        let sh = self.sh.then_some(Format::Sh);
        let asked: Vec<Format> = [self.format, toml, sh].into_iter().flatten().collect();
        match asked[..] {
            [] => Ok(by_default),
            [format] => Ok(format),
            _ => Err(Error::Usage(
                "give at most one of --format, --toml, and --sh".to_owned(),
            )),
        }
    }
}

#[derive(Debug, Args)]
pub struct CleanArgs {
    #[command(subcommand)]
    pub mode: Mode,

    #[command(flatten)]
    pub flags: ScanFlags,
}

/// `rosie check <path>…` takes the flags of `scan`, without `--sh`
/// (`docs/spec/cli.md#check`).
#[derive(Debug, Args)]
pub struct CheckArgs {
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,

    #[command(flatten)]
    pub flags: ScanFlags,

    #[command(flatten)]
    pub rules: RuleFlags,
}

#[derive(Debug, Clone, Default, Args)]
pub struct ScanFlags {
    /// Mark aggressive items `clean`; `cmd_aggressive` replaces `cmd`.
    #[arg(long, global = true)]
    pub aggressive: bool,

    /// Walk across volumes (overrides `[walk] enter_mounts`).
    #[arg(long, global = true)]
    pub enter_mounts: bool,

    /// `none`, `roots`, or `all` (overrides `[walk] enter_links`).
    #[arg(long, global = true, value_name = "LEVEL", value_parser = parse_enter_links)]
    pub enter_links: Option<EnterLinks>,
}

impl ScanFlags {
    /// How the plan lists aggressive items: `clean` with `--aggressive`, else `keep`.
    pub fn aggressive_items(&self) -> AggressiveItems {
        match self.aggressive {
            true => AggressiveItems::Clean,
            false => AggressiveItems::Keep,
        }
    }

    /// The settings every scan mode starts from, and the ones `app` and `orphans` scan
    /// with: the config's `[walk]` settings, with `enter_mounts` turned on by
    /// `--enter-mounts` and `enter_links` replaced by `--enter-links`
    /// (`docs/spec/safety.md#walk-skips`).
    pub fn settings(&self, walk: Walk) -> Settings {
        let enter_links = self.enter_links.unwrap_or(walk.enter_links);
        let walk = Walk {
            enter_mounts: walk.enter_mounts || self.enter_mounts,
            enter_links,
            ..walk
        };
        Settings {
            walk,
            aggressive: self.aggressive_items(),
        }
    }
}

#[derive(Debug, Clone, Subcommand)]
pub enum Mode {
    /// Everything the rules match under <dir>, the current folder by default.
    Tree {
        dir: Option<PathBuf>,
        #[command(flatten)]
        rules: RuleFlags,
    },

    /// Tool-wide caches.
    Caches {
        #[command(flatten)]
        rules: RuleFlags,
    },

    /// An application and its leftovers.
    App { app: Option<PathBuf> },

    /// Leftovers of apps no longer installed.
    Orphans,
}

impl Default for Mode {
    /// Bare `rosie` is `rosie clean tree .`.
    fn default() -> Self {
        Mode::Tree {
            dir: None,
            rules: RuleFlags::default(),
        }
    }
}

impl Mode {
    /// The flags that would open the skipped path, or `None` when this mode does not
    /// take every one of them (`docs/spec/cli.md#flags`). Every mode takes
    /// `--enter-mounts`; only `tree` and `caches` take the bundle and placeholder flags. A
    /// sealed skip stays closed whatever the flags, and `Denied` has no flag in any mode.
    pub fn walk_skip_hint(&self, skip: WalkSkip) -> Option<Hint> {
        let WalkSkip::Closed(needs) = skip else {
            return None;
        };
        let rule_driven = matches!(self, Mode::Tree { .. } | Mode::Caches { .. });
        let accepted = |setting: WalkSetting| match setting {
            WalkSetting::Mounts => true,
            WalkSetting::Placeholders | WalkSetting::Bundles => rule_driven,
        };
        if !needs.settings().all(accepted) {
            return None;
        }
        Some(enter_hint(needs))
    }
}

/// The flags that open a folder closed for `needs`, all at once, named by what the
/// outermost one does: `to cross it: --enter-mounts`.
pub(super) fn enter_hint(needs: Needs) -> Hint {
    let label = match needs.settings().count() {
        1 => format!("to {} it", verb(needs.outermost())),
        _ => "to enter it".to_owned(),
    };
    Hint {
        label,
        steps: needs
            .settings()
            .map(|setting| Step::Flag(Flag::Enter(setting)))
            .collect(),
    }
}

/// What `setting` lets the walk do to the folder it closes.
fn verb(setting: WalkSetting) -> &'static str {
    match setting {
        WalkSetting::Placeholders => "open",
        WalkSetting::Mounts => "cross",
        WalkSetting::Bundles => "enter",
    }
}

/// The flags of the rule-driven modes, `tree` and `caches`. `app` and `orphans` apply no
/// rules and take only the mounts setting of `[walk]`, so they do not accept these.
#[derive(Debug, Clone, Default, Args)]
pub struct RuleFlags {
    /// Limit the run to this rule name, in any pack; repeatable. See `rosie rules`.
    #[arg(long = "only", value_name = "RULE", value_parser = parse_name)]
    pub only: Vec<Name>,

    /// Walk into bundles (overrides `[walk] enter_bundles`).
    #[arg(long)]
    pub enter_bundles: bool,

    /// Open dataless cloud placeholders (overrides `[walk] enter_placeholders`).
    #[arg(long)]
    pub enter_placeholders: bool,
}

impl RuleFlags {
    /// `walk`, with `enter_bundles` and `enter_placeholders` each turned on by its flag.
    pub fn applied_to(&self, walk: Walk) -> Walk {
        Walk {
            enter_bundles: walk.enter_bundles || self.enter_bundles,
            enter_placeholders: walk.enter_placeholders || self.enter_placeholders,
            ..walk
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanSource {
    Stdin,
    File(PathBuf),
}

#[derive(Debug, Subcommand)]
pub enum RulesAction {
    /// Install a rule pack from a GitHub owner/repo; chakrit/rosie by default.
    Pull { source: Option<String> },
    /// Remove a pulled pack.
    Remove { pack: Option<String> },
}

#[derive(Debug, Subcommand)]
pub enum RootsAction {
    /// Add a path to the `roots` allowlist.
    Add { path: PathBuf },
    /// Remove a path from the `roots` allowlist.
    Remove { path: Option<PathBuf> },
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print a config value.
    Get { key: Option<String> },
    /// Set a config value.
    Set {
        key: Option<String>,
        value: Option<String>,
    },
    /// Delete the key from config.toml so its default applies.
    Unset { key: Option<String> },
}

fn parse_plan_source(text: &str) -> Result<PlanSource, String> {
    match text {
        "-" => Ok(PlanSource::Stdin),
        path => Ok(PlanSource::File(PathBuf::from(path))),
    }
}

fn parse_name(text: &str) -> Result<Name, String> {
    Name::parse(text).map_err(|error| error.to_string())
}

fn parse_enter_links(text: &str) -> Result<EnterLinks, String> {
    EnterLinks::parse(text).ok_or_else(|| {
        let levels: Vec<&str> = EnterLinks::ALL.into_iter().map(EnterLinks::label).collect();
        format!("expected one of: {}", levels.join(", "))
    })
}

fn parse_format(text: &str) -> Result<Format, String> {
    Format::ALL
        .into_iter()
        .find(|format| format.label() == text)
        .ok_or_else(|| {
            let formats: Vec<&str> = Format::ALL.into_iter().map(Format::label).collect();
            format!("expected one of: {}", formats.join(", "))
        })
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::hint::Command as HintCommand;

    fn every_command() -> Vec<HintCommand> {
        let mut commands = vec![
            HintCommand::Roots,
            HintCommand::RootsAdd(PathBuf::from("/Users/me/My Code")),
            HintCommand::RootsAddAny,
            HintCommand::RootsRemove(PathBuf::from("~/.cargo/registry")),
            HintCommand::RootsRemovePick,
            HintCommand::Config,
            HintCommand::Rules,
            HintCommand::RulesPull,
            HintCommand::RulesRemove("extra".to_owned()),
            HintCommand::ScanTree(PathBuf::from("~/src")),
            HintCommand::Rerun(vec!["clean".to_owned(), "tree".to_owned(), ".".to_owned()]),
        ];
        commands.extend(EnterLinks::ALL.map(HintCommand::ConfigSetLinks));
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
}
