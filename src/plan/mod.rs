//! The cleanup plan: what a scan found, what the user confirms, and what a run executes
//! (`docs/spec/plan.md`).
//!
//! A plan is built by [`PlanBuilder`] during a scan or parsed back from its TOML file
//! ([`Plan::parse`]); nothing can add entries to a parsed plan. A run executes only a
//! [`Runnable`], whose fields are private: the only ways to get one are
//! [`Plan::runnable_as`] and [`Plan::after_elevation`], which yield only the `clean`
//! entries of one part of a run, and a `Runnable` can only be narrowed from there.
//! `skip` and `conflict` entries are kept for the user to see, never
//! executed.

mod builder;
mod conflict;
mod deletes;
mod error;
mod file;
mod script;
mod size;
mod stats;
mod trim;

use std::fmt;
use std::path::{Path, PathBuf};

use crate::fs::{Argv, FileKind, ShellCommand, SystemArgv, SystemTool};
use crate::hint::{self, Hint};

use deletes::Deletes;

pub use builder::{AggressiveItems, AgreedTiers, Built, PathMatch, PlanBuilder, Reach};
pub use conflict::Disagreement;
pub use error::{EntryProblem, Error};
pub use size::Size;
pub use stats::{Counted, ItemSize, PlanStats, Tally};
pub use trim::CleanEntry;

/// Everything one scan proposes, grouped by what each entry does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    deletes: Deletes,
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

    /// What the part of a run acting as `run_as` executes: the `clean` entries of that
    /// part (see [`Plan::clean_entries_for`]). A bootout runs with the part that carries
    /// its delete, and every bootout must finish before the deletes start, so a launch job
    /// is unloaded before its plist is gone (`docs/spec/app.md`). A user's delete that
    /// holds a `system` job's plist is in the elevated part for its bootouts only; it
    /// is deleted by [`Plan::after_elevation`] (see [`Delete::phase`]).
    pub fn runnable_as(&self, run_as: RunAs) -> Runnable<'_> {
        let holders: Vec<&Delete> = self
            .deletes
            .iter()
            .filter(|delete| delete.runs_in(run_as))
            .collect();
        Runnable {
            deletes: holders
                .iter()
                .copied()
                .filter(|delete| delete.run_as == run_as)
                .collect(),
            holders,
            tools: self.tools.iter().filter(|t| t.runs_in(run_as)).collect(),
            receipts: self.receipts.iter().filter(|r| r.runs_in(run_as)).collect(),
        }
    }

    /// The user's `clean` deletes of the elevated part, which the user's own process
    /// deletes once the elevated child has booted their `system` jobs out. They carry
    /// no bootout, tool, or receipt.
    pub fn after_elevation(&self) -> Runnable<'_> {
        Runnable {
            holders: Vec::new(),
            deletes: self
                .deletes
                .iter()
                .filter(|delete| delete.runs_in(RunAs::Sudo) && delete.run_as == RunAs::User)
                .collect(),
            tools: Vec::new(),
            receipts: Vec::new(),
        }
    }

    /// The `clean` entries a run executes as `run_as`, as a plan of their own: a run
    /// does the user's part itself and pipes the `sudo` part to the elevated child
    /// (`docs/spec/safety.md#elevation`). Tools run as the user; receipts as root.
    pub fn clean_entries_for(&self, run_as: RunAs) -> Plan {
        let deletes = self.deletes.retaining(|delete| delete.runs_in(run_as));
        let tools = self.tools.iter().filter(|tool| tool.runs_in(run_as));
        let receipts = self
            .receipts
            .iter()
            .filter(|receipt| receipt.runs_in(run_as));

        Plan {
            deletes,
            tools: tools.cloned().collect(),
            receipts: receipts.cloned().collect(),
            reports: Vec::new(),
        }
    }

    /// Whether the plan holds no entry that could run.
    pub fn is_empty(&self) -> bool {
        self.deletes.is_empty() && self.tools.is_empty() && self.receipts.is_empty()
    }
}

/// What a `skip` entry's comment says: `skip: aggressive`, or `skip: outside the roots`
/// with the hints that would let it in on the lines below.
fn skip_note(reason: SkipReason, path: &Path) -> String {
    match reason {
        SkipReason::Aggressive => "skip: aggressive".to_owned(),
        SkipReason::OutsideRoots => {
            let hints = hint::hints_text(&Hint::allow_in_roots(path));
            format!("skip: outside the roots\n{hints}")
        }
        SkipReason::Chosen => "skip".to_owned(),
    }
}

/// `text` as comment lines, so no line of it can run or parse as TOML.
fn comment(text: &str) -> String {
    text.lines().map(|line| format!("# {line}\n")).collect()
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

/// The `clean` entries of a plan that one process of a run executes, which are the only
/// ones a run may execute. Only [`Plan::runnable_as`] and [`Plan::after_elevation`]
/// build one; [`Runnable::without`] can only leave entries out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runnable<'a> {
    /// The `clean` deletes whose launch jobs this process boots out.
    holders: Vec<&'a Delete>,
    /// The `clean` deletes this process deletes.
    deletes: Vec<&'a Delete>,
    tools: Vec<&'a Tool>,
    receipts: Vec<&'a Receipt>,
}

impl<'a> Runnable<'a> {
    /// The deletes whose bootouts this process runs. In the elevated child they include
    /// the user's deletes that hold a `system` job's plist, which the child boots out
    /// but does not delete.
    pub fn holders(&self) -> &[&'a Delete] {
        &self.holders
    }

    /// Every bootout of [`Runnable::holders`], in plan order.
    pub fn bootouts(&self) -> impl Iterator<Item = &'a Bootout> + '_ {
        self.holders
            .iter()
            .flat_map(|delete| delete.bootouts.iter())
    }

    pub fn deletes(&self) -> &[&'a Delete] {
        &self.deletes
    }

    pub fn tools(&self) -> &[&'a Tool] {
        &self.tools
    }

    pub fn receipts(&self) -> &[&'a Receipt] {
        &self.receipts
    }

    /// The same entries without the deletes `refused` selects, which lose their
    /// bootouts with them.
    pub fn without(&self, refused: impl Fn(&Delete) -> bool) -> Runnable<'a> {
        let kept = |deletes: &[&'a Delete]| {
            deletes
                .iter()
                .copied()
                .filter(|delete| !refused(delete))
                .collect()
        };
        Runnable {
            holders: kept(&self.holders),
            deletes: kept(&self.deletes),
            tools: self.tools.clone(),
            receipts: self.receipts.clone(),
        }
    }
}

// entries

/// A file or folder to delete through the roots gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delete {
    pub path: PathBuf,
    /// What `lstat` reported at scan time; a run skips the entry as stale when it changed.
    pub kind: ItemKind,
    pub size: Size,
    /// Every rule that matched this path.
    pub rules: Vec<String>,
    pub status: Status,
    pub run_as: RunAs,
    /// The launch jobs whose plists are this path or lie inside it, one per plist. They
    /// share this entry's status, so a plist is never deleted with its job still loaded.
    pub bootouts: Vec<Bootout>,
}

impl Delete {
    /// Which part of a run carries this delete and its bootouts: the elevated part when
    /// the delete or any of its bootouts needs root. A delete that runs as the user but
    /// holds a `system` job's plist goes to the elevated part for its bootouts only; the
    /// user's own process deletes it once the elevated part is done. So a plist is never
    /// deleted before its job is booted out, and root never deletes an entry that runs as
    /// the user.
    pub fn phase(&self) -> RunAs {
        let bootout_needs_root = self.bootouts.iter().any(|b| b.run_as() == RunAs::Sudo);
        match (self.run_as, bootout_needs_root) {
            (RunAs::User, false) => RunAs::User,
            _ => RunAs::Sudo,
        }
    }

    /// Whether this delete is `clean` and belongs to the part of a run acting as `run_as`.
    fn runs_in(&self, run_as: RunAs) -> bool {
        self.status.is_clean() && self.phase() == run_as
    }
}

/// `launchctl bootout <domain> <plist>`, run before the delete that holds the plist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bootout {
    pub plist: PathBuf,
    pub domain: LaunchDomain,
}

impl Bootout {
    /// The command as a run executes it: the [`SystemTool`] by its absolute path, so no
    /// `PATH` lookup can put another program in its place.
    pub fn absolute_argv(&self) -> SystemArgv {
        SystemTool::LAUNCHCTL
            .argv()
            .arg("bootout")
            .arg(self.domain.to_string())
            .arg(&self.plist)
    }

    /// The command with `launchctl` as its program, as the shell export writes it.
    pub(super) fn argv_for(&self, launchctl: &str) -> Argv {
        Argv::new(launchctl)
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

/// One tool rule's own cleanup action, outside the roots gate. Its size is unknown.
/// Rosie never compares two tools' commands: each entry is its rule's own action
/// (`docs/spec/plan.md#plan-file`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub rule: String,
    pub command: ShellCommand,
    pub selection: Selection,
}

impl Tool {
    /// Whether this tool is `clean` and belongs to the part acting as `run_as`: tools run
    /// as the user.
    fn runs_in(&self, run_as: RunAs) -> bool {
        self.selection.is_clean() && run_as == RunAs::User
    }
}

/// `pkgutil --forget <package>`, a tool command whose size is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub package: Package,
    pub selection: Selection,
}

impl Receipt {
    /// The command as a run executes it: the [`SystemTool`] by its absolute path, so no
    /// `PATH` lookup can put another program in its place.
    pub fn absolute_argv(&self) -> SystemArgv {
        SystemTool::PKGUTIL
            .argv()
            .arg("--forget")
            .arg(self.package.as_str())
    }

    /// The command with `pkgutil` as its program, as the shell export writes it.
    pub(super) fn argv_for(&self, pkgutil: &str) -> Argv {
        Argv::new(pkgutil)
            .arg("--forget")
            .arg(self.package.as_str())
    }

    /// Receipts live in root-owned folders.
    pub fn run_as(&self) -> RunAs {
        RunAs::Sudo
    }

    /// Whether this receipt is `clean` and belongs to the part acting as `run_as`.
    fn runs_in(&self, run_as: RunAs) -> bool {
        self.selection.is_clean() && self.run_as() == run_as
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

/// What a deletable path was when scanned. Symlinks never match rules, so a plan never
/// holds one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    File,
    Folder,
}

impl ItemKind {
    pub const ALL: [ItemKind; 2] = [ItemKind::File, ItemKind::Folder];

    pub fn label(self) -> &'static str {
        match self {
            ItemKind::File => "file",
            ItemKind::Folder => "folder",
        }
    }

    /// The kind a plan entry's `type` names.
    pub fn parse(label: &str) -> Option<ItemKind> {
        ItemKind::ALL
            .into_iter()
            .find(|candidate| candidate.label() == label)
    }

    /// Whether `lstat` still reports this kind.
    pub fn matches(self, kind: FileKind) -> bool {
        matches!(
            (self, kind),
            (ItemKind::File, FileKind::File) | (ItemKind::Folder, FileKind::Dir)
        )
    }
}

/// Whether a filesystem entry runs (`docs/spec/plan.md#statuses`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Clean,
    Skip(SkipReason),
    /// Rules disagree on its tier, and no one has answered `clean` or `skip` yet
    /// (`docs/spec/plan.md#rule-conflicts`).
    Conflict(Sides),
}

/// What a conflict knows of its rules' tiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sides {
    /// From the scan that found it.
    Scanned(Disagreement),
    /// Read back from a plan file, whose comment alone names the tiers; `line` is where
    /// the entry's `status` sits in the file.
    Read { line: usize },
}

impl Status {
    pub fn label(&self) -> &'static str {
        self.kind().label()
    }

    pub fn kind(&self) -> StatusKind {
        match self {
            Status::Clean => StatusKind::Clean,
            Status::Skip(_) => StatusKind::Skip,
            Status::Conflict(_) => StatusKind::Conflict,
        }
    }

    pub fn is_clean(&self) -> bool {
        *self == Status::Clean
    }
}

/// Why a scan made an entry `skip`. The plan file writes it as a comment only, so an
/// entry read back from a file is [`SkipReason::Chosen`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// An aggressive item, scanned without `--aggressive`.
    Aggressive,
    /// Outside the `roots` allowlist.
    OutsideRoots,
    /// Answered `skip` by the user, at a prompt, with `--conflicts`, or in a plan file.
    Chosen,
}

/// The words a plan file's `status` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Clean,
    Skip,
    Conflict,
}

impl StatusKind {
    pub const ALL: [StatusKind; 3] = [StatusKind::Clean, StatusKind::Skip, StatusKind::Conflict];

    pub fn label(self) -> &'static str {
        match self {
            StatusKind::Clean => "clean",
            StatusKind::Skip => "skip",
            StatusKind::Conflict => "conflict",
        }
    }

    /// The kind a plan entry's `status` names.
    pub fn parse(label: &str) -> Option<StatusKind> {
        StatusKind::ALL
            .into_iter()
            .find(|candidate| candidate.label() == label)
    }
}

/// Whether a command entry runs. Commands are outside the roots gate and never conflict,
/// so a scan skips one only as aggressive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Clean,
    Skip,
}

impl Selection {
    pub fn label(self) -> &'static str {
        match self {
            Selection::Clean => StatusKind::Clean.label(),
            Selection::Skip => StatusKind::Skip.label(),
        }
    }

    /// The status a conflict answered `self` takes.
    pub fn answered(self) -> Status {
        match self {
            Selection::Clean => Status::Clean,
            Selection::Skip => Status::Skip(SkipReason::Chosen),
        }
    }
}

impl Selection {
    pub fn is_clean(self) -> bool {
        self == Selection::Clean
    }
}

/// Who a run executes an entry as (`docs/spec/safety.md#elevation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAs {
    User,
    /// Done after the user's items, in the one elevated child.
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

/// `text` as a tool rule's `cmd` for the default shell, with no added environment, for
/// tests.
#[cfg(test)]
fn tool_command(text: &str) -> ShellCommand {
    let shell = ShellCommand::DEFAULT_SHELL.to_owned();
    ShellCommand::new(shell, text.to_owned(), std::collections::BTreeMap::new())
        .expect("a valid test command")
}
