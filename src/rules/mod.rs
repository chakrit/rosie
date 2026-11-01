//! Cleanup rules: the TOML rule model, its layers, the folder-name index, and the Lua
//! strategy (`docs/spec/rules.md`).

mod error;
mod layers;
mod lua;
mod pack;
mod rule;
mod set;
pub mod table;

pub use error::Error;
pub use layers::{PackFolder, RuleLayers};
pub use lua::LuaSandbox;
pub use pack::load_pack;
pub use rule::{
    Detection, FixedPath, FixedPaths, FolderName, FolderRule, Globs, LuaCode, Markers, Rule, Shape,
    Source,
};
pub use set::{Candidate, RuleSet, TargetRule};
