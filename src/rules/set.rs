//! The loaded rules and their folder-name index (`docs/spec/performance.md#scan`).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;

use super::error::Error;
use super::layers::{self, RuleDirs};
use super::lua::LuaSandbox;
use super::name::RuleId;
use super::rule::{Detection, Rule, Shape, Tier};
use crate::fs::{Backend, Gate};

/// Every rule from every layer, in identity order, with the folder rules indexed once
/// by target name.
pub struct RuleSet {
    rules: Box<[Rule]>,
    by_target: HashMap<OsString, Box<[TargetRule]>>,
}

impl RuleSet {
    /// Loads the pulled packs, then the user's rules over them.
    pub fn load<B: Backend>(gate: &Gate<B>, dirs: &RuleDirs) -> Result<RuleSet, Error> {
        let rules = layers::load(gate, dirs)?;
        Ok(RuleSet::index(rules))
    }

    fn index(rules: Vec<Rule>) -> RuleSet {
        let mut by_target: HashMap<OsString, Vec<TargetRule>> = HashMap::new();
        for rule in &rules {
            let Shape::Folder(folder) = &rule.shape else {
                continue;
            };
            for (tier, target) in folder.target.tiers() {
                let entry = TargetRule {
                    id: rule.id.clone(),
                    tier,
                    detection: folder.detection.clone(),
                };
                let name = target.as_os_str().to_owned();
                by_target.entry(name).or_default().push(entry);
            }
        }

        let by_target = by_target
            .into_iter()
            .map(|(name, entries)| (name, entries.into_boxed_slice()))
            .collect();
        RuleSet {
            rules: rules.into(),
            by_target,
        }
    }

    /// Every rule, in `pack/rule` order, for `rosie rules`.
    pub fn iter(&self) -> impl Iterator<Item = &Rule> {
        self.rules.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The folder rules whose `target` or `target_aggressive` is exactly `name`.
    pub fn targeting(&self, name: &OsStr) -> &[TargetRule] {
        self.by_target.get(name).map_or(&[], |entries| entries)
    }
}

/// A folder the walk found, with the names listed beside it.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    /// The folder, already `lstat`ed by the walk as a folder, not a symlink.
    pub path: &'a Path,
    /// The names in the folder's parent, the folder's own included.
    pub siblings: &'a [OsString],
}

/// A folder rule under one of its target names, with the tier that target came from.
#[derive(Debug, Clone)]
pub struct TargetRule {
    id: RuleId,
    tier: Tier,
    detection: Detection,
}

impl TargetRule {
    pub fn id(&self) -> &RuleId {
        &self.id
    }

    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// Whether the rule's detection accepts a candidate whose name it targets. An error
    /// (a failed listing, a Lua error) is reported against the rule; the candidate then
    /// does not match.
    pub fn matches<B: Backend>(
        &self,
        gate: &Gate<B>,
        lua: &LuaSandbox,
        candidate: Candidate,
    ) -> Result<bool, Error> {
        match &self.detection {
            Detection::Name => Ok(true),
            Detection::Lua(code) => lua.accepts(gate, &self.id, code, candidate.path),
            Detection::Marker(markers) => {
                let own_name = candidate.path.file_name();
                let siblings = candidate
                    .siblings
                    .iter()
                    .map(OsString::as_os_str)
                    .filter(|&name| Some(name) != own_name);
                let sibling_ok = markers
                    .sibling()
                    .is_none_or(|globs| globs.match_any(siblings));
                if !sibling_ok {
                    return Ok(false);
                }

                let Some(inside) = markers.inside() else {
                    return Ok(true);
                };
                let listing = gate
                    .read_dir(candidate.path)
                    .map_err(|source| Error::Listing {
                        rule: self.id.clone(),
                        path: candidate.path.to_path_buf(),
                        source: Box::new(source),
                    })?;
                Ok(inside.match_any(listing.iter().map(OsString::as_os_str)))
            }
        }
    }
}
