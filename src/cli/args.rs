//! The command line, exactly as `docs/spec/cli.md` lays it out. Flags parse into the
//! plain values clap needs; `super` turns them into typed settings once.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::plan::{WalkSetting, WalkSkip};
use crate::rules::Name;
use crate::run::elevated;

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

    /// Print the plan as a shell script to inspect and run yourself.
    #[arg(long, global = true)]
    pub sh: bool,
}

#[derive(Debug, Args)]
pub struct CleanArgs {
    #[command(subcommand)]
    pub mode: Mode,

    #[command(flatten)]
    pub flags: ScanFlags,
}

#[derive(Debug, Clone, Default, Args)]
pub struct ScanFlags {
    /// Tick aggressive items; `cmd_aggressive` replaces `cmd`.
    #[arg(long, global = true)]
    pub aggressive: bool,

    /// Walk across volumes (overrides `[walk] enter_mounts`).
    #[arg(long, global = true)]
    pub enter_mounts: bool,
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
    /// The flags that would open the skipped path, worded for the parenthetical a
    /// walk-skip line adds, or `None` when this mode does not take every one of them
    /// (`docs/spec/cli.md#flags`). Every mode takes `--enter-mounts`; only `tree` and
    /// `caches` take the bundle and placeholder flags. A sealed skip stays closed
    /// whatever the flags, and `Denied` has no flag in any mode.
    pub fn walk_skip_hint(&self, skip: WalkSkip) -> Option<String> {
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

        let flags: Vec<&str> = needs.settings().map(flag).collect();
        let outermost = needs.outermost();
        let hint = match flags.as_slice() {
            [] | [_] => format!("use {} to {}", flag(outermost), verb(outermost)),
            [init @ .., last] => format!("use {} and {last} to enter", init.join(", ")),
        };
        Some(hint)
    }
}

/// The command-line flag that turns `setting` on.
fn flag(setting: WalkSetting) -> &'static str {
    match setting {
        WalkSetting::Placeholders => "--enter-placeholders",
        WalkSetting::Mounts => "--enter-mounts",
        WalkSetting::Bundles => "--enter-bundles",
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
