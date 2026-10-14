//! A failed command as the user reads it (`docs/spec/cli.md#errors`): each problem, then
//! the hints for it. Every error variant is matched by name, with no catch-all, so a new
//! variant does not compile until it is given its hints or none.

use std::path::{Path, PathBuf};

use super::error::Error;
use super::root::RootError;
use super::session::PickKind;
use crate::config::{self, UnusableRoot};
use crate::fs::{self, EnterLinks, RealPath};
use crate::hint::{Command, Hint};
use crate::run::elevated;
use crate::{apps, packs, plan, rules, scan};

/// One problem and what the user can do about it.
#[derive(Debug)]
pub struct Failure {
    pub problem: String,
    pub hints: Vec<Hint>,
}

impl Failure {
    fn new(problem: impl ToString, hints: Vec<Hint>) -> Failure {
        Failure {
            problem: problem.to_string(),
            hints,
        }
    }
}

/// The problems `error` reports: one, or one per unusable root.
pub fn failures(error: &Error) -> Vec<Failure> {
    let config_error = match error {
        Error::Config(error) => Some(error),
        Error::Elevated(elevated::Error::Config(error)) => Some(error),
        _ => None,
    };
    match config_error {
        Some(config::Error::UnusableRoots(roots)) => roots.iter().map(unusable_root).collect(),
        Some(_) | None => vec![Failure::new(error, hints(error))],
    }
}

fn hints(error: &Error) -> Vec<Hint> {
    match error {
        Error::NothingToPick { kind } => nothing_to_pick_hints(*kind),
        Error::Fs(error) => fs_hints(error),
        Error::Config(error) => config_hints(error),
        Error::Packs(error) => packs_hints(error),
        Error::Rules(error) => rules_hints(error),
        Error::Scan(error) => scan_hints(error),
        Error::Apps(error) => apps_hints(error),
        Error::Plan(error) => plan_hints(error),
        Error::Elevated(error) => elevated_hints(error),
        Error::Root(RootError::Refused(_) | RootError::Process(_))
        | Error::Usage(_)
        | Error::Cancelled
        | Error::PlanNotText(_)
        | Error::Io(_) => Vec::new(),
    }
}

/// A `roots` entry that fails here, named as written: keep it by allowing what it passes
/// through, or replace it with its real location.
fn unusable_root(root: &UnusableRoot) -> Failure {
    let UnusableRoot { written, error } = root;
    let replace = |label: &str, real: &Path| {
        Hint::run(
            label,
            [
                Command::RootsRemove(written.clone()),
                Command::RootsAdd(real.to_path_buf()),
            ],
        )
    };
    let hints = match error {
        fs::Error::Symlink {
            real: RealPath::Resolved(real),
            ..
        } => vec![allow_links_in_roots(), replace("use the real path", real)],
        fs::Error::Symlink {
            real: RealPath::Unresolved(_),
            ..
        } => vec![allow_links_in_roots(), remove_root(written)],
        fs::Error::Misspelled { real, .. } => vec![replace("use the spelling on disk", real)],
        other => [fs_hints(other), vec![remove_root(written)]].concat(),
    };
    Failure::new(format!("root {}: {error}", written.display()), hints)
}

fn allow_links_in_roots() -> Hint {
    Hint::run(
        "allow symlinks in roots",
        [Command::ConfigSetLinks(EnterLinks::Roots)],
    )
}

fn remove_root(written: &Path) -> Hint {
    Hint::run("remove it", [Command::RootsRemove(written.to_path_buf())])
}

fn nothing_to_pick_hints(kind: PickKind) -> Vec<Hint> {
    match kind {
        PickKind::Root => vec![Hint::run("add one", [Command::RootsAddAny])],
        PickKind::Pack => vec![Hint::run("pull the default pack", [Command::RulesPull])],
        PickKind::App | PickKind::Key | PickKind::Value => Vec::new(),
    }
}

fn fs_hints(error: &fs::Error) -> Vec<Hint> {
    match error {
        fs::Error::OutsideRoots { path } => {
            vec![Hint::run("allow it", [Command::RootsAdd(path.clone())])]
        }
        fs::Error::Symlink { real, .. } => {
            let allow = Hint::run(
                "allow symlinks in roots and typed paths",
                [Command::ConfigSetLinks(EnterLinks::Roots)],
            );
            match real {
                RealPath::Resolved(real) => vec![allow, use_instead(real)],
                RealPath::Unresolved(_) => vec![allow],
            }
        }
        fs::Error::Misspelled { real, .. } => vec![use_instead(real)],
        fs::Error::Running { program, .. } => vec![Hint::by_hand(format!(
            "quit {}, then run again",
            program_name(program)
        ))],
        fs::Error::Io { source, .. } if source.kind() == std::io::ErrorKind::PermissionDenied => {
            vec![full_disk_access()]
        }
        fs::Error::Io { .. }
        | fs::Error::Command { .. }
        | fs::Error::NotAnEntryName { .. }
        | fs::Error::Relative { .. }
        | fs::Error::CrossesVolume { .. }
        | fs::Error::Placeholder { .. }
        | fs::Error::LinkDangling { .. }
        | fs::Error::LinkLoop { .. }
        | fs::Error::RunningUnknown { .. }
        | fs::Error::NotAFolder { .. }
        | fs::Error::OutsideOwnData { .. }
        | fs::Error::OwnFolderItself { .. } => Vec::new(),
    }
}

fn use_instead(real: &Path) -> Hint {
    Hint::by_hand(format!("use {} instead", real.display()))
}

fn full_disk_access() -> Hint {
    Hint::by_hand(
        "give your terminal Full Disk Access in System Settings > Privacy & Security > \
         Full Disk Access, then run again",
    )
}

/// `Slack` for `/Applications/Slack.app/Contents/MacOS/Slack`.
fn program_name(program: &Path) -> String {
    let name = program.file_name().unwrap_or(program.as_os_str());
    name.to_string_lossy().into_owned()
}

fn config_hints(error: &config::Error) -> Vec<Hint> {
    let edit = |path: &PathBuf| vec![Hint::by_hand(format!("edit {}", path.display()))];
    match error {
        config::Error::Fs(error) => fs_hints(error),
        config::Error::UnknownKey { .. } => {
            vec![Hint::run("see the keys", [Command::Config])]
        }
        config::Error::RootsNotSettable => vec![
            Hint::run("add a root", [Command::RootsAddAny]),
            Hint::run("remove one", [Command::RootsRemovePick]),
        ],
        config::Error::NotARoot { .. } => vec![Hint::run("see the roots", [Command::Roots])],
        config::Error::NotUtf8 { path, .. }
        | config::Error::Parse { path, .. }
        | config::Error::InvalidShape { path, .. } => edit(path),
        // Each unusable root is its own failure; see `failures`.
        config::Error::UnusableRoots(_)
        | config::Error::NotFound
        | config::Error::InvalidDefault(_)
        | config::Error::InvalidValue { .. }
        | config::Error::RootNotUtf8 { .. } => Vec::new(),
    }
}

fn packs_hints(error: &packs::Error) -> Vec<Hint> {
    match error {
        packs::Error::Fs(error) => fs_hints(error),
        packs::Error::BadPullTime { .. } => {
            vec![Hint::run("refresh the pack", [Command::RulesPull])]
        }
        packs::Error::PackNameTaken { installed, .. } => vec![Hint::run(
            "remove the installed one first",
            [Command::RulesRemove(installed.pack().to_string())],
        )],
        packs::Error::NotInstalled { .. } => {
            vec![Hint::run("see the installed packs", [Command::Rules])]
        }
        packs::Error::StrayEntry { path, .. } => vec![Hint::by_hand(format!(
            "move {} out of the packs folder",
            path.display()
        ))],
        packs::Error::BadSource { .. }
        | packs::Error::ReservedPack { .. }
        | packs::Error::Download { .. }
        | packs::Error::Archive(_)
        | packs::Error::Unsafe { .. }
        | packs::Error::OutsideTopFolder { .. }
        | packs::Error::Duplicate { .. }
        | packs::Error::TooLarge { .. }
        | packs::Error::UnpackedTooLarge { .. }
        | packs::Error::NoRules
        | packs::Error::NoConfig
        | packs::Error::ConfigNotText
        | packs::Error::InvalidRules(_)
        | packs::Error::Restore { .. }
        | packs::Error::Leftover { .. }
        | packs::Error::Ambiguous { .. } => Vec::new(),
    }
}

fn rules_hints(error: &rules::Error) -> Vec<Hint> {
    match error {
        rules::Error::Fs(error) => fs_hints(error),
        rules::Error::UnknownRule { .. } => {
            vec![Hint::run("see the rule names", [Command::Rules])]
        }
        rules::Error::Toml { .. }
        | rules::Error::Rule { .. }
        | rules::Error::DuplicatePack { .. }
        | rules::Error::DuplicateRule { .. }
        | rules::Error::UnknownOverride { .. }
        | rules::Error::WrongKind { .. }
        | rules::Error::LuaStart(_)
        | rules::Error::Lua { .. }
        | rules::Error::Listing { .. } => Vec::new(),
    }
}

fn scan_hints(error: &scan::Error) -> Vec<Hint> {
    match error {
        scan::Error::Fs(error) | scan::Error::PathRefused(error) => fs_hints(error),
        scan::Error::NotAFolder { .. } | scan::Error::Missing { .. } => Vec::new(),
    }
}

fn apps_hints(error: &apps::Error) -> Vec<Hint> {
    match error {
        apps::Error::Fs(error) | apps::Error::Refused { source: error, .. } => fs_hints(error),
        apps::Error::Plan(error) => plan_hints(error),
        apps::Error::Process(_)
        | apps::Error::NotAnApp { .. }
        | apps::Error::NoInfoPlist { .. }
        | apps::Error::MissingKey { .. }
        | apps::Error::InvalidBundleId { .. }
        | apps::Error::UnreadableApp { .. }
        | apps::Error::UnreadableBundle { .. }
        | apps::Error::UnlistableAppFolder { .. }
        | apps::Error::LaunchJobs { .. }
        | apps::Error::SystemJobs { .. }
        | apps::Error::SystemJobsMissing
        | apps::Error::Placeholder { .. }
        | apps::Error::Mount { .. }
        | apps::Error::Item(_) => Vec::new(),
    }
}

fn plan_hints(error: &plan::Error) -> Vec<Hint> {
    match error {
        plan::Error::UnresolvedConflicts { .. } => vec![Hint::by_hand(
            "save the plan as TOML and set each \"conflict\" status to clean or keep",
        )],
        plan::Error::NulByte { .. }
        | plan::Error::ConflictingLaunchDomain { .. }
        | plan::Error::RelativePath { .. }
        | plan::Error::NonUtf8Path { .. }
        | plan::Error::DotComponent { .. }
        | plan::Error::EmptyPackage
        | plan::Error::EmptySubject
        | plan::Error::MissingVersion
        | plan::Error::UnsupportedVersion { .. }
        | plan::Error::NestedDeletes { .. }
        | plan::Error::RepeatedDelete { .. }
        | plan::Error::InvalidEntry { .. }
        | plan::Error::Parse(_)
        | plan::Error::Serialize(_) => Vec::new(),
    }
}

fn elevated_hints(error: &elevated::Error) -> Vec<Hint> {
    match error {
        elevated::Error::Fs(error) => fs_hints(error),
        elevated::Error::Config(error) => config_hints(error),
        elevated::Error::NotAPipe
        | elevated::Error::Header(_)
        | elevated::Error::NonceMismatch
        | elevated::Error::NotRoot { .. }
        | elevated::Error::NoSudoAncestor
        | elevated::Error::ToolElevated
        | elevated::Error::Process(_)
        | elevated::Error::Plan(_)
        | elevated::Error::Outcomes(_) => Vec::new(),
    }
}
