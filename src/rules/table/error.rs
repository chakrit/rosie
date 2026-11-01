use thiserror::Error;

use crate::fs::InvalidCommand;
use crate::{name, wildcard};

/// Why one rule table is invalid (`docs/spec/rules.md#rule-tables`).
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Name(#[from] name::Error),

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

    #[error("`{field}` path {value:?} must start with `/`, or be `~` or start with `~/`")]
    BadPath { field: &'static str, value: String },

    #[error("`{field}` repeats the path {value:?}")]
    DuplicatePath { field: &'static str, value: String },

    #[error("`{field}`: {source}")]
    BadGlob {
        field: &'static str,
        #[source]
        source: wildcard::Error,
    },

    #[error("`{field}`: {source}")]
    BadCommand {
        field: &'static str,
        #[source]
        source: InvalidCommand,
    },

    #[error("Lua does not compile: {0}")]
    LuaSyntax(String),
}
