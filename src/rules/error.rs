use std::path::PathBuf;

use thiserror::Error;

use super::table;
use crate::fs;
use crate::name::{Pattern, RuleId};

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
        problem: Box<table::Error>,
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

    #[error("no rule matches `{pattern}`")]
    NoRuleMatches { pattern: Pattern },
}
