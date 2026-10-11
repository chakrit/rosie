//! The loaded rule model (`docs/spec/rules.md#rule-tables`). The TOML is checked once,
//! in `parse`.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::PathBuf;

use super::glob::Glob;
use super::name::RuleId;
use crate::fs::{Paths, ShellCommand};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub(super) id: RuleId,
    pub(super) source: Source,
    pub(super) shape: Shape,
}

impl Rule {
    pub fn id(&self) -> &RuleId {
        &self.id
    }

    pub fn source(&self) -> Source {
        self.source
    }

    pub fn shape(&self) -> &Shape {
        &self.shape
    }
}

/// Where the rule that holds an identity came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Defined in its own pack: a pulled pack, or `user` for the user's own rules.
    Pack,
    /// A user rule file replacing the pack's rule by its qualified name.
    UserOverride,
}

/// What a rule cleans and how it finds it. The strategy follows from the shape, and so
/// do the modes that use it (`docs/spec/rules.md#modes`): the scanner dispatches on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// Folders found by the walk: the `name`, `marker`, and `lua` strategies.
    Folder(FolderRule),
    /// The `path` strategy.
    Paths(Twin<FixedPaths>),
    /// The `tool` strategy: `cmd` and `cmd_aggressive` share the rule's `shell` and `env`.
    Tool(Twin<ShellCommand>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRule {
    pub target: Twin<FolderName>,
    pub detection: Detection,
}

/// How a folder whose name matches `target` is confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection {
    /// The name alone.
    Name,
    Marker(Markers),
    Lua(LuaCode),
}

/// Sibling files, files inside the folder, or both; both must match when both are set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Markers {
    Sibling(Globs),
    Inside(Globs),
    Both { sibling: Globs, inside: Globs },
}

impl Markers {
    pub(super) fn from_parts(sibling: Option<Globs>, inside: Option<Globs>) -> Option<Markers> {
        match (sibling, inside) {
            (Some(sibling), Some(inside)) => Some(Markers::Both { sibling, inside }),
            (Some(sibling), None) => Some(Markers::Sibling(sibling)),
            (None, Some(inside)) => Some(Markers::Inside(inside)),
            (None, None) => None,
        }
    }

    pub fn sibling(&self) -> Option<&Globs> {
        match self {
            Markers::Sibling(sibling) | Markers::Both { sibling, .. } => Some(sibling),
            Markers::Inside(_) => None,
        }
    }

    pub fn inside(&self) -> Option<&Globs> {
        match self {
            Markers::Inside(inside) | Markers::Both { inside, .. } => Some(inside),
            Markers::Sibling(_) => None,
        }
    }
}

/// A non-empty any-of list of glob patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Globs(Box<[Glob]>);

impl Globs {
    pub(super) fn new(globs: Vec<Glob>) -> Option<Globs> {
        match globs.is_empty() {
            true => None,
            false => Some(Globs(globs.into())),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Glob> {
        self.0.iter()
    }

    /// Whether any pattern matches any of the names. Names that are not UTF-8 match
    /// nothing; APFS stores only UTF-8 names.
    pub fn match_any<'a>(&self, names: impl IntoIterator<Item = &'a OsStr>) -> bool {
        names
            .into_iter()
            .filter_map(OsStr::to_str)
            .any(|name| self.0.iter().any(|glob| glob.matches(name)))
    }
}

/// A Lua detection: `expr` is wrapped as `return (...)`, `script` is a full chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LuaCode {
    Expr(Box<str>),
    Script(Box<str>),
}

impl LuaCode {
    /// The chunk Lua runs. The newline keeps a trailing `--` comment in an `expr` from
    /// swallowing the closing parenthesis.
    pub fn chunk(&self) -> Cow<'_, str> {
        match self {
            LuaCode::Expr(expr) => Cow::Owned(format!("return ({expr}\n)")),
            LuaCode::Script(script) => Cow::Borrowed(script),
        }
    }
}

/// One exact folder name: no `/`, and not `.` or `..`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderName(Box<str>);

impl FolderName {
    pub(super) fn parse(text: &str) -> Option<FolderName> {
        let valid = !text.is_empty() && !text.contains(['/', '\0']) && text != "." && text != "..";
        valid.then(|| FolderName(text.into()))
    }

    pub fn as_os_str(&self) -> &OsStr {
        OsStr::new(&*self.0)
    }
}

/// A non-empty list of fixed paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedPaths(Box<[FixedPath]>);

impl FixedPaths {
    pub(super) fn new(paths: Vec<FixedPath>) -> Option<FixedPaths> {
        match paths.is_empty() {
            true => None,
            false => Some(FixedPaths(paths.into())),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &FixedPath> {
        self.0.iter()
    }
}

/// A fixed path as a rule writes it, anchored at `/` or at the home folder by `~` or
/// `~/…`. `roots` decides what it may reach (`docs/spec/rules.md#paths`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedPath(PathBuf);

impl FixedPath {
    /// The written path, or `None` when it is anchored at neither `/` nor `~`.
    pub(super) fn parse(text: &str) -> Option<FixedPath> {
        let anchored = text.starts_with('/') || text == "~" || text.starts_with("~/");
        anchored.then(|| FixedPath(PathBuf::from(text)))
    }

    /// The absolute path, made by [`Paths::expand`].
    pub fn resolve(&self, paths: &Paths) -> PathBuf {
        paths
            .expand(&self.0)
            .expect("a fixed path is anchored at `/` or `~`, so it expands")
    }
}

/// A "what to clean" value and its `_aggressive` twin; at least one is set
/// (`docs/spec/rules.md#aggressive-twins`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Twin<T> {
    Normal(T),
    Aggressive(T),
    Both { normal: T, aggressive: T },
}

impl<T> Twin<T> {
    pub(super) fn from_parts(normal: Option<T>, aggressive: Option<T>) -> Option<Twin<T>> {
        match (normal, aggressive) {
            (Some(normal), Some(aggressive)) => Some(Twin::Both { normal, aggressive }),
            (Some(normal), None) => Some(Twin::Normal(normal)),
            (None, Some(aggressive)) => Some(Twin::Aggressive(aggressive)),
            (None, None) => None,
        }
    }

    pub fn normal(&self) -> Option<&T> {
        match self {
            Twin::Normal(normal) | Twin::Both { normal, .. } => Some(normal),
            Twin::Aggressive(_) => None,
        }
    }

    pub fn aggressive(&self) -> Option<&T> {
        match self {
            Twin::Aggressive(aggressive) | Twin::Both { aggressive, .. } => Some(aggressive),
            Twin::Normal(_) => None,
        }
    }

    /// Each value set, with the tier it belongs to.
    pub fn tiers(&self) -> impl Iterator<Item = (Tier, &T)> {
        let normal = self.normal().map(|value| (Tier::Normal, value));
        let aggressive = self.aggressive().map(|value| (Tier::Aggressive, value));
        normal.into_iter().chain(aggressive)
    }
}

/// Whether an item comes from a plain field or its `_aggressive` twin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Normal,
    Aggressive,
}
