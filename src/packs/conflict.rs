//! Rules from different packs that clean the same thing.
//!
//! Two folder rules clash when their target and detection are equal
//! (`docs/spec/rules.md#packs`), `_aggressive` twins included: the rule model's
//! `FolderRule` carries both.
//! Two `path` rules clash when they share a path once `~` is resolved, and two `tool`
//! rules when they share a command, in either tier. Rules of different shapes never
//! clash.

use std::fmt;
use std::path::PathBuf;

use crate::fs::{Argv, Home};
use crate::rules::{
    self, Detection, FixedPaths, FolderRule, Glob, Globs, Markers, Rule, RuleId, Shape, Twin,
};

use super::archive::RuleFile;
use super::error::Error;
use super::source::Source;

/// A freshly pulled rule that duplicates a rule of another installed pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The freshly pulled rule.
    pub pulled: RuleId,
    /// The already-installed rule it duplicates.
    pub installed: RuleId,
}

impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} and {} clean the same target the same way",
            self.pulled, self.installed
        )
    }
}

/// A pack's rules, loaded as the rule layers load them.
pub(super) struct PackRules {
    rules: Vec<Rule>,
}

impl PackRules {
    /// Loads every rule file of a pack with `rules::load_pack`, so a pack refused here is
    /// exactly a pack the rule layers would refuse.
    pub(super) fn load(source: &Source, files: &[RuleFile]) -> Result<PackRules, Error> {
        let labeled: Vec<(PathBuf, &[u8])> = files
            .iter()
            .map(|file| (file_label(source, file), file.contents()))
            .collect();

        let rules = rules::load_pack(
            source.pack(),
            labeled
                .iter()
                .map(|(label, bytes)| (label.as_path(), *bytes)),
        )
        .map_err(|source| Error::InvalidRules(Box::new(source)))?;
        Ok(PackRules { rules })
    }

    /// The rules of `self` that duplicate a rule of `other`, with `~` taken as `home`.
    pub(super) fn conflicts_with(&self, other: &PackRules, home: &Home) -> Vec<Conflict> {
        self.rules
            .iter()
            .flat_map(|pulled| {
                other
                    .rules
                    .iter()
                    .filter(move |installed| clash(pulled.shape(), installed.shape(), home))
                    .map(move |installed| Conflict {
                        pulled: pulled.id().clone(),
                        installed: installed.id().clone(),
                    })
            })
            .collect()
    }
}

fn clash(pulled: &Shape, installed: &Shape, home: &Home) -> bool {
    match (pulled, installed) {
        (Shape::Folder(pulled), Shape::Folder(installed)) => folder_clash(pulled, installed),
        (Shape::Paths(pulled), Shape::Paths(installed)) => {
            let installed = resolved(installed, home);
            resolved(pulled, home)
                .iter()
                .any(|path| installed.contains(path))
        }
        (Shape::Tool(pulled), Shape::Tool(installed)) => {
            let installed = commands(installed);
            commands(pulled)
                .iter()
                .any(|command| installed.contains(command))
        }
        (Shape::Folder(_) | Shape::Paths(_) | Shape::Tool(_), _) => false,
    }
}

/// Whether two folder rules detect the same folder: the same target, and detection
/// that names the same marker globs regardless of the order they were written in.
fn folder_clash(pulled: &FolderRule, installed: &FolderRule) -> bool {
    pulled.target == installed.target && detection_clash(&pulled.detection, &installed.detection)
}

fn detection_clash(pulled: &Detection, installed: &Detection) -> bool {
    match (pulled, installed) {
        (Detection::Name, Detection::Name) => true,
        (Detection::Lua(pulled), Detection::Lua(installed)) => pulled == installed,
        (Detection::Marker(pulled), Detection::Marker(installed)) => {
            markers_clash(pulled, installed)
        }
        (Detection::Name | Detection::Lua(_) | Detection::Marker(_), _) => false,
    }
}

fn markers_clash(pulled: &Markers, installed: &Markers) -> bool {
    same_globs(pulled.sibling(), installed.sibling())
        && same_globs(pulled.inside(), installed.inside())
}

/// Whether two optional glob lists name the same patterns, ignoring order.
fn same_globs(pulled: Option<&Globs>, installed: Option<&Globs>) -> bool {
    match (pulled, installed) {
        (None, None) => true,
        (Some(pulled), Some(installed)) => {
            let mut pulled: Vec<&str> = pulled.iter().map(Glob::as_str).collect();
            let mut installed: Vec<&str> = installed.iter().map(Glob::as_str).collect();
            pulled.sort_unstable();
            installed.sort_unstable();
            pulled == installed
        }
        (None, Some(_)) | (Some(_), None) => false,
    }
}

/// Every path of both tiers, with `~` taken as `home`.
fn resolved(paths: &Twin<FixedPaths>, home: &Home) -> Vec<PathBuf> {
    paths
        .tiers()
        .flat_map(|(_, paths)| paths.iter())
        .map(|path| path.resolve(home))
        .collect()
}

/// The command of both tiers. The parser already split each on spaces, so commands that
/// differ only in surrounding or repeated spaces are equal here.
fn commands(cmd: &Twin<Argv>) -> Vec<&Argv> {
    cmd.tiers().map(|(_, argv)| argv).collect()
}

fn file_label(source: &Source, file: &RuleFile) -> PathBuf {
    PathBuf::from(format!("{source}/{}", file.name().to_string_lossy()))
}
