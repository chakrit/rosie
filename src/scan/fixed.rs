//! Fixed rule paths as scan targets (`docs/spec/rules.md#paths`).
//!
//! Each path is resolved with the injected home, so two spellings of one path, such as
//! `/Users/me/.npm` and `~/.npm`, are one target listing both rules. A path through a
//! symlink hits the symlink ban and is reported like any refused path; an absent path
//! is simply not planned.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use rayon::Scope;

use super::RuleMatch;
use super::entry::{Entry, NotAnEntry};
use super::log::{Log, Problem};
use super::progress::Progress;
use super::targets::Targets;
use crate::config::Walk;
use crate::fs::{self, Backend, Gate, resolve_dots};
use crate::plan::WalkSkip;
use crate::rules::{FixedPath, RuleId, RuleSet, Shape, Tier};

/// Every fixed rule path, resolved, with the rules that list it.
pub(super) struct FixedTargets(BTreeMap<PathBuf, Vec<RuleMatch>>);

impl FixedTargets {
    pub fn of(rules: &RuleSet, home: &Path, log: &Log) -> Self {
        let mut targets: BTreeMap<PathBuf, Vec<RuleMatch>> = BTreeMap::new();
        for (rule, tier, fixed) in fixed_paths(rules) {
            let path = match resolve_dots(&fixed.resolve(home)) {
                Ok(path) => path,
                Err(error) => {
                    log.problem(Problem::Path(error));
                    continue;
                }
            };

            let matched = RuleMatch {
                rule: rule.clone(),
                tier,
            };
            let listed = targets.entry(path).or_default();
            if !listed.contains(&matched) {
                listed.push(matched);
            }
        }
        FixedTargets(targets)
    }

    /// Only the paths at or below `root`.
    pub fn under(self, root: &Path) -> Self {
        let below = self
            .0
            .into_iter()
            .filter(|(path, _)| path.starts_with(root))
            .collect();
        FixedTargets(below)
    }

    /// Only the outermost paths: one inside another collapses into it, as the walk
    /// would never reach it past a match.
    pub fn outermost(self) -> Self {
        let mut kept: BTreeMap<PathBuf, Vec<RuleMatch>> = BTreeMap::new();
        for (path, rules) in self.0 {
            // `Path` orders by component, so an enclosing path sorts just before.
            let enclosed = kept
                .last_key_value()
                .is_some_and(|(outer, _)| path.starts_with(outer));
            if !enclosed {
                kept.insert(path, rules);
            }
        }
        FixedTargets(kept)
    }

    /// Reports each path that passes through a symlink. The walk never enters a link,
    /// so it would pass such a path by without a word.
    pub fn report_symlinked<B: Backend>(&self, gate: &Gate<B>, log: &Log) {
        for path in self.0.keys() {
            if let Err(error @ fs::Error::Symlink { .. }) = gate.lstat_link_free(path) {
                log.problem(Problem::Path(error));
            }
        }
    }

    /// The paths, for the walk to look each entry up in.
    pub fn into_map(self) -> HashMap<PathBuf, Vec<RuleMatch>> {
        self.0.into_iter().collect()
    }

    pub fn into_iter(self) -> impl Iterator<Item = (PathBuf, Vec<RuleMatch>)> {
        self.0.into_iter()
    }
}

/// Checks one fixed path for `caches` and claims it. A denied path is a walk skip and
/// an absent one is not planned, as in the walk. A dataless placeholder is never
/// planned; a path on another volume than its folder is a mount, skipped unless the
/// walk may enter mounts.
pub(super) fn claim<'s, 'a: 's, B: Backend + Sync, P: Progress>(
    gate: &Gate<B>,
    targets: &'s Targets<'a, B, P>,
    log: &Log,
    flags: Walk,
    scope: &Scope<'s>,
    path: PathBuf,
    rules: Vec<RuleMatch>,
) {
    let entry = match Entry::lstat(gate, &path) {
        Ok(entry) => entry,
        Err(NotAnEntry::Placeholder { path }) => return log.skip(path, WalkSkip::Placeholder),
        Err(NotAnEntry::Fs(error)) => return log.entry_failed(path, error),
    };
    if !flags.enter_mounts {
        match is_mount(gate, &entry) {
            Ok(true) => return log.skip(path, WalkSkip::Mount),
            Ok(false) => {}
            Err(error) => return log.problem(Problem::Path(error)),
        }
    }

    targets.claim(scope, entry, rules);
}

/// Every fixed path of a `path` rule, with the tier of the field that lists it.
fn fixed_paths(rules: &RuleSet) -> impl Iterator<Item = (&RuleId, Tier, &FixedPath)> {
    rules.iter().flat_map(|rule| {
        let tiers = match rule.shape() {
            Shape::Paths(paths) => Some(paths.tiers()),
            Shape::Folder(_) | Shape::Tool(_) => None,
        };
        tiers
            .into_iter()
            .flatten()
            .flat_map(move |(tier, paths)| paths.iter().map(move |path| (rule.id(), tier, path)))
    })
}

/// Whether the entry sits on another volume than the folder holding it.
fn is_mount<B: Backend>(gate: &Gate<B>, entry: &Entry) -> Result<bool, fs::Error> {
    let Some(folder) = entry.path().parent() else {
        return Ok(false);
    };
    let folder_meta = gate.lstat(folder)?;
    Ok(folder_meta.dev != entry.meta().dev)
}
