//! The loaded rule model (`docs/spec/rules.md#rule-tables`). Every value here is valid
//! by construction; the TOML is checked once, in `parse`.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::PathBuf;

use super::glob::Glob;
use super::name::RuleId;
use crate::fs::{Argv, Home};

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

/// What a rule cleans and how it finds it. The strategy follows from the shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// Folders found by the walk: the `name`, `marker`, and `lua` strategies.
    Folder(FolderRule),
    /// The `path` strategy.
    Paths(Twin<FixedPaths>),
    /// The `tool` strategy.
    Tool(Twin<Argv>),
}

impl Shape {
    /// The modes that use a rule of this shape (`docs/spec/rules.md#modes`). `tree`
    /// takes only the fixed paths that lie under its folder.
    pub fn modes(&self) -> &'static [Mode] {
        match self {
            Shape::Folder(_) => &[Mode::Tree],
            Shape::Paths(_) => &[Mode::Caches, Mode::Tree],
            Shape::Tool(_) => &[Mode::Caches],
        }
    }
}

/// A scan mode that rules feed. `app` and `orphans` are scanning logic, not rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Tree,
    Caches,
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

/// A fixed path as a rule writes it: absolute, or under the home folder via `~`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixedPath {
    Absolute(PathBuf),
    /// The non-empty path below the home folder, with only normal components: never
    /// empty, since `~` and `~/` alone are load errors.
    Home(PathBuf),
}

impl FixedPath {
    pub(super) fn parse(text: &str) -> Result<FixedPath, PathProblem> {
        let home = match text {
            "~" => Some(""),
            _ => text.strip_prefix("~/"),
        };
        match (home, text.strip_prefix('/')) {
            (Some(rest), _) => rest_to_path(rest, || FixedPath::Home(PathBuf::from(rest))),
            (None, Some(rest)) => rest_to_path(rest, || FixedPath::Absolute(PathBuf::from(text))),
            (None, None) => Err(PathProblem::NotAnchored),
        }
    }

    /// The absolute path, with `~` taken as the injected home folder. It has no `.` or
    /// `..`: neither spelling admits one and home has its own resolved.
    pub fn resolve(&self, home: &Home) -> PathBuf {
        match self {
            FixedPath::Absolute(path) => path.clone(),
            FixedPath::Home(rest) => home.join(rest),
        }
    }
}

/// Why a fixed path was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PathProblem {
    /// Neither absolute nor `~`/`~/…`.
    NotAnchored,
    /// An empty, `.`, or `..` component (including a trailing slash, which leaves a
    /// trailing empty component).
    BadComponent,
    /// Exactly `/` or `~`: the root or home folder itself.
    RootOrHome,
}

/// Builds a fixed path from the text after the anchor (`~/` or `/`), refusing an empty
/// remainder (the anchor alone) and any `.`, `..`, or empty component, so the remainder
/// never resolves to a different path than the one written.
fn rest_to_path(rest: &str, make: impl FnOnce() -> FixedPath) -> Result<FixedPath, PathProblem> {
    if rest.is_empty() {
        return Err(PathProblem::RootOrHome);
    }
    let normal = rest
        .split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
    normal.then(make).ok_or(PathProblem::BadComponent)
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

    /// The same twin with `convert` applied to each value set.
    pub fn map<U>(&self, convert: impl Fn(&T) -> U) -> Twin<U> {
        match self {
            Twin::Normal(normal) => Twin::Normal(convert(normal)),
            Twin::Aggressive(aggressive) => Twin::Aggressive(convert(aggressive)),
            Twin::Both { normal, aggressive } => Twin::Both {
                normal: convert(normal),
                aggressive: convert(aggressive),
            },
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
