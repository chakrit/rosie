use std::path::PathBuf;

use thiserror::Error;

use super::glob::GlobError;
use super::name::{Name, NameError, RuleId};
use super::parse::COMMAND_PUNCTUATION;
use crate::fs;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error("{file}: {source}", file = file.display())]
    Toml {
        file: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error("{file}: rule `{rule}`: {problem}", file = file.display())]
    Rule {
        file: PathBuf,
        /// The rule's key as written in the file.
        rule: String,
        #[source]
        problem: Problem,
    },

    #[error(
        "the pack `{pack}` is installed twice, at {first} and {second}",
        first = first.display(),
        second = second.display(),
    )]
    DuplicatePack {
        pack: String,
        first: PathBuf,
        second: PathBuf,
    },

    #[error(
        "rule {rule} is defined twice, in {first} and {second}",
        first = first.display(),
        second = second.display(),
    )]
    DuplicateRule {
        rule: RuleId,
        first: PathBuf,
        second: PathBuf,
    },

    #[error("{file}: rule `{rule}` overrides a rule no pack defines", file = file.display())]
    UnknownOverride { file: PathBuf, rule: RuleId },

    #[error("{path} is a {found}, not a {expected}", path = path.display())]
    WrongKind {
        path: PathBuf,
        found: &'static str,
        expected: &'static str,
    },

    #[error("cannot start Lua: {0}")]
    LuaStart(String),

    #[error("rule {rule} failed on {path}: {message}", path = path.display())]
    Lua {
        rule: RuleId,
        path: PathBuf,
        message: String,
    },

    #[error("rule {rule} could not list {path}: {source}", path = path.display())]
    Listing {
        rule: RuleId,
        path: PathBuf,
        #[source]
        source: Box<fs::Error>,
    },

    #[error("no pack has a rule named `{name}`; `rosie rules` lists them")]
    UnknownRule { name: Name },
}

/// Why one rule table is invalid.
#[derive(Debug, Error)]
pub enum Problem {
    #[error("{0}")]
    Name(#[from] NameError),

    #[error("an override by qualified name belongs in the user rules, not in a pack")]
    OverrideInPack,

    #[error("is not a table")]
    NotATable,

    #[error("has no `strategy`")]
    MissingStrategy,

    #[error("has an unknown strategy {0:?}; use name, marker, lua, path, or tool")]
    UnknownStrategy(String),

    #[error("`{field}` does not apply to the {strategy} strategy")]
    FieldNotAllowed {
        field: String,
        strategy: &'static str,
    },

    #[error("{0}")]
    Invalid(#[source] Box<toml::de::Error>),

    #[error("needs {0}")]
    Missing(&'static str),

    #[error("`{0}` cannot be an empty list")]
    EmptyList(&'static str),

    #[error("needs exactly one of `expr` or `script`, not both")]
    BothExprAndScript,

    #[error("`{field}` = {value:?} is not one folder name")]
    BadFolderName { field: &'static str, value: String },

    #[error("`{field}` path {value:?} must be absolute or start with `~/`")]
    BadPath { field: &'static str, value: String },

    #[error(
        "`{field}` path {value:?} has an empty, `.`, or `..` component (a trailing slash \
         leaves one too)"
    )]
    BadPathComponent { field: &'static str, value: String },

    #[error("`{field}` path {value:?} is the root or home folder itself")]
    RootOrHome { field: &'static str, value: String },

    #[error("`{field}` repeats the path {value:?}")]
    DuplicatePath { field: &'static str, value: String },

    #[error("`{normal}` and `{aggressive}` must not share a value")]
    TwinOverlap {
        normal: &'static str,
        aggressive: &'static str,
    },

    #[error("`{field}`: {source}")]
    BadGlob {
        field: &'static str,
        #[source]
        source: GlobError,
    },

    #[error("`{0}` is empty")]
    EmptyCommand(&'static str),

    #[error(
        "`{field}` = {value:?}: the character {character:?} is not allowed; a command runs \
         without a shell, split on spaces, and holds only ASCII letters, digits, spaces, \
         and {punctuation}",
        punctuation = COMMAND_PUNCTUATION,
    )]
    CommandCharacter {
        field: &'static str,
        value: String,
        character: char,
    },

    #[error("Lua does not compile: {0}")]
    LuaSyntax(String),
}
