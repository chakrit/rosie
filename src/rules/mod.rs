//! Cleanup rules: the TOML rule model, its layers, the folder-name index, and the Lua
//! strategy (`docs/spec/rules.md`).

mod error;
mod glob;
mod layers;
mod lua;
mod name;
mod pack;
mod parse;
mod rule;
mod set;

pub use error::{Error, Problem};
pub use glob::{Glob, GlobError};
pub use layers::RuleDirs;
pub use lua::LuaSandbox;
pub use name::{Name, NameError, RuleId};
pub use pack::load_pack;
pub use rule::{
    Detection, FixedPath, FixedPaths, FolderName, FolderRule, Globs, LuaCode, Markers, Mode, Rule,
    Shape, Source, Tier, Twin,
};
pub use set::{Candidate, RuleSet, TargetRule};
