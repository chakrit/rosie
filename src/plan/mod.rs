//! The cleanup plan: what a scan found, what the user confirms, and what a run executes
//! (`docs/spec/plan.md`).
//!
//! A plan is built by [`PlanBuilder`] during a scan or parsed back from its TOML file
//! ([`Plan::parse`]); nothing else can create one, and nothing can add entries to a
//! parsed plan. Only [`Plan::runnable`] hands entries to a run, and it yields ticked
//! entries alone: unticked and blocked entries are kept for the user to see, never
//! executed.

mod builder;
mod error;
mod file;
mod quote;
mod script;
mod size;
mod stats;

use std::fmt;
use std::path::{Path, PathBuf};

use crate::fs::{Argv, FileKind};

pub use builder::{AggressiveItems, PathMatch, PlanBuilder, Reach, ToolCmds, Twin};
pub use error::Error;
pub use file::FORMAT_VERSION;
pub use size::Size;
pub use stats::{ItemSize, Outcome, PlanStats, RunStats, SkipReason, Tally, WalkSkip, WalkSkips};

/// Everything one scan proposes, grouped by what each entry does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// A scan leaves these sorted by path, with no entry inside another; a plan file is
    /// read back in its own order.
    deletes: Vec<Delete>,
    tools: Vec<Tool>,
    receipts: Vec<Receipt>,
    reports: Vec<Report>,
}

impl Plan {
    pub fn deletes(&self) -> &[Delete] {
        &self.deletes
    }

    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    pub fn receipts(&self) -> &[Receipt] {
        &self.receipts
    }

    pub fn reports(&self) -> &[Report] {
        &self.reports
    }

    /// The ticked entries a run executes. A bootout runs exactly when the delete that
    /// holds its plist does, and every bootout must finish before the deletes start, so
    /// a launch job is unloaded before its plist is gone (`docs/spec/app.md`).
    pub fn runnable(&self) -> Runnable<'_> {
        let deletes: Vec<&Delete> = self
            .deletes
            .iter()
            .filter(|d| d.status.is_ticked())
            .collect();
        Runnable {
            bootouts: deletes
                .iter()
                .flat_map(|delete| delete.bootouts.iter())
                .collect(),
            deletes,
            tools: self
                .tools
                .iter()
                .filter(|t| t.selection.is_ticked())
                .collect(),
            receipts: self
                .receipts
                .iter()
                .filter(|r| r.selection.is_ticked())
                .collect(),
        }
    }
}

/// What a blocked entry shows: `blocked, outside roots. To allow it: rosie roots add
/// <path>`, with the path quoted to paste into a shell.
fn blocked_hint(path: &Path) -> String {
    let path = quote::word(&path.to_string_lossy());
    format!("blocked, outside roots. To allow it: rosie roots add {path}")
}

/// A NUL byte ends a path or an argument at the operating system, so the item a run
/// reached would not be the item the plan lists.
fn refuse_nul(text: &str) -> Result<(), Error> {
    match text.contains('\0') {
        true => Err(Error::NulByte {
            text: text.to_owned(),
        }),
        false => Ok(()),
    }
}

/// A plan holds only absolute UTF-8 paths without NUL bytes or `.` and `..` components.
/// A `.` or `..` lets a path name another item than the one it reads as, and breaks
/// collapsing, which compares paths component by component.
fn check_path(path: &Path) -> Result<(), Error> {
    if !path.is_absolute() {
        return Err(Error::RelativePath { path: path.into() });
    }
    let Some(text) = path.to_str() else {
        return Err(Error::NonUtf8Path { path: path.into() });
    };
    refuse_nul(text)?;

    let dotted = text
        .split('/')
        .any(|component| component == "." || component == "..");
    match dotted {
        true => Err(Error::DotComponent { path: path.into() }),
        false => Ok(()),
    }
}

/// The ticked entries of a plan, which are the only ones a run may execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runnable<'a> {
    pub bootouts: Vec<&'a Bootout>,
    pub deletes: Vec<&'a Delete>,
    pub tools: Vec<&'a Tool>,
    pub receipts: Vec<&'a Receipt>,
}

// entries

/// A file or folder to delete through the roots gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delete {
    pub path: PathBuf,
    /// What `lstat` reported at scan time; a run skips the entry as stale when it changed.
    pub kind: ItemKind,
    pub size: Size,
    /// Every rule that matched this path, sorted and without repeats.
    pub rules: Vec<String>,
    pub status: Status,
    pub run_as: RunAs,
    /// The launch jobs whose plists are this path or lie inside it, one per plist. They
    /// share this entry's status, so a plist is never deleted with its job still loaded.
    pub bootouts: Vec<Bootout>,
}

/// `launchctl bootout <domain> <plist>`, run before the delete that holds the plist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bootout {
    pub plist: PathBuf,
    pub domain: LaunchDomain,
}

impl Bootout {
    pub fn argv(&self) -> Argv {
        Argv::new("launchctl")
            .arg("bootout")
            .arg(self.domain.to_string())
            .arg(&self.plist)
    }

    /// The `system` domain needs root; a user's `gui/<uid>` domain does not.
    pub fn run_as(&self) -> RunAs {
        match self.domain {
            LaunchDomain::Gui(_) => RunAs::User,
            LaunchDomain::System => RunAs::Sudo,
        }
    }
}

/// A tool's own cleanup command, outside the roots gate. Its size is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    /// Every rule whose trimmed command is this one.
    pub rules: Vec<String>,
    pub command: Command,
    pub selection: Selection,
}

/// `pkgutil --forget <package>`, a tool command whose size is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub package: Package,
    pub selection: Selection,
}

impl Receipt {
    pub fn argv(&self) -> Argv {
        Argv::new("pkgutil")
            .arg("--forget")
            .arg(self.package.as_str())
    }

    /// Receipts live in root-owned folders.
    pub fn run_as(&self) -> RunAs {
        RunAs::Sudo
    }
}

/// Something rosie cannot remove itself, such as a login item or a system extension,
/// with the steps the user takes by hand. Never executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    subject: String,
    steps: Vec<String>,
}

impl Report {
    /// A report names what it is about, so its subject is never empty.
    pub fn new(subject: String, steps: Vec<String>) -> Result<Self, Error> {
        if subject.is_empty() {
            return Err(Error::EmptySubject);
        }
        Ok(Report { subject, steps })
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn steps(&self) -> &[String] {
        &self.steps
    }
}

// entry values

/// A package id for `pkgutil --forget`: never empty, and without NUL bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package(String);

impl Package {
    pub fn new(id: String) -> Result<Self, Error> {
        if id.is_empty() {
            return Err(Error::EmptyPackage);
        }
        refuse_nul(&id)?;
        Ok(Package(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A program and its arguments, never run through a shell. Never empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    program: String,
    args: Vec<String>,
}

impl Command {
    /// Splits rule `rule`'s `cmd` on whitespace (`docs/spec/rules.md#tool-strategy`). A
    /// `cmd` holding a quote or a backslash is refused rather than split into words its
    /// author did not mean.
    pub fn split(rule: &str, cmd: &str) -> Result<Self, Error> {
        if cmd.contains(['"', '\'', '\\']) {
            return Err(Error::QuotedCommand {
                rule: rule.to_owned(),
                cmd: cmd.to_owned(),
            });
        }

        let words = cmd.split_whitespace().map(str::to_owned).collect();
        Command::from_words(words).map_err(|error| match error {
            Error::EmptyArgv => Error::EmptyCommand {
                rule: rule.to_owned(),
            },
            other => other,
        })
    }

    /// A command from an argv list, such as one read back from a plan file.
    pub fn from_words(words: Vec<String>) -> Result<Self, Error> {
        for word in &words {
            refuse_nul(word)?;
        }

        let mut words = words.into_iter();
        let program = words.next().ok_or(Error::EmptyArgv)?;
        Ok(Command {
            program,
            args: words.collect(),
        })
    }

    pub fn words(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.program.as_str()).chain(self.args.iter().map(String::as_str))
    }

    pub fn argv(&self) -> Argv {
        self.args
            .iter()
            .fold(Argv::new(&self.program), |argv, arg| argv.arg(arg))
    }
}

/// What a deletable path was when scanned. Symlinks never match rules, so a plan never
/// holds one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    File,
    Folder,
}

impl ItemKind {
    pub fn label(self) -> &'static str {
        match self {
            ItemKind::File => "file",
            ItemKind::Folder => "folder",
        }
    }

    /// Whether `lstat` still reports this kind.
    pub fn matches(self, kind: FileKind) -> bool {
        matches!(
            (self, kind),
            (ItemKind::File, FileKind::File) | (ItemKind::Folder, FileKind::Dir)
        )
    }
}

/// Whether a filesystem entry runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ticked,
    /// An aggressive item listed without `--aggressive`.
    Unticked,
    /// Outside the `roots` allowlist; shown with a `rosie roots add <path>` hint.
    Blocked,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Ticked => "ticked",
            Status::Unticked => "unticked",
            Status::Blocked => "blocked",
        }
    }

    pub fn is_ticked(self) -> bool {
        self == Status::Ticked
    }
}

impl From<Selection> for Status {
    fn from(selection: Selection) -> Self {
        match selection {
            Selection::Ticked => Status::Ticked,
            Selection::Unticked => Status::Unticked,
        }
    }
}

/// Whether a command entry runs. Commands are outside the roots gate, so they are never
/// blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Ticked,
    Unticked,
}

impl Selection {
    pub fn is_ticked(self) -> bool {
        self == Selection::Ticked
    }
}

/// Who a run executes an entry as (`docs/spec/safety.md#elevation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAs {
    User,
    /// Not owned by the user; done after the user's items, in the one elevated child.
    Sudo,
}

/// The `launchctl` domain a job is booted out of (`docs/spec/app.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchDomain {
    /// A user LaunchAgent: `gui/<uid>`.
    Gui(u32),
    /// A daemon in `/Library/LaunchDaemons`.
    System,
}

impl LaunchDomain {
    pub fn parse(text: &str) -> Option<Self> {
        if text == "system" {
            return Some(LaunchDomain::System);
        }
        let uid = text.strip_prefix("gui/")?;
        let all_digits = !uid.is_empty() && uid.bytes().all(|byte| byte.is_ascii_digit());
        match all_digits {
            true => uid.parse().ok().map(LaunchDomain::Gui),
            false => None,
        }
    }
}

impl fmt::Display for LaunchDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchDomain::Gui(uid) => write!(f, "gui/{uid}"),
            LaunchDomain::System => f.write_str("system"),
        }
    }
}
