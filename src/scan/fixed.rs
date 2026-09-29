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
use super::entry::{Entry, Mounts, NotAnEntry};
use super::log::{Log, Problem};
use super::progress::Progress;
use super::targets::Targets;
use crate::fs::{self, Backend, Gate, Home};
use crate::plan::WalkSkip;
use crate::rules::{FixedPath, RuleId, RuleSet, Shape, Tier};

/// Every fixed rule path, resolved, with the rules that list it.
pub(super) struct FixedTargets(BTreeMap<PathBuf, Vec<RuleMatch>>);

impl FixedTargets {
    pub fn of(rules: &RuleSet, home: &Home) -> Self {
        let mut targets: BTreeMap<PathBuf, Vec<RuleMatch>> = BTreeMap::new();
        for (rule, tier, fixed) in fixed_paths(rules) {
            let path = fixed.resolve(home);

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

/// Claims fixed paths for `caches`, checking each as the walk would have checked it on
/// the way down. A denied path is a walk skip and an absent one is not planned. A
/// dataless placeholder is never planned. A path on another volume than its folder is
/// a mount root and is never planned, even with `enter_mounts`: the gate refuses to
/// delete a mount point at all. Without `enter_mounts`, a path inside a mount is not
/// planned either: one reached through a folder on another volume than the folder
/// above it, counting from home for a path under home and from `/` otherwise.
pub(super) struct FixedClaim<'a, B: Backend, P: Progress> {
    pub gate: &'a Gate<B>,
    pub home: &'a Home,
    pub mounts: Mounts,
    pub targets: &'a Targets<'a, B, P>,
    pub log: &'a Log,
}

impl<B: Backend + Sync, P: Progress> FixedClaim<'_, B, P> {
    pub fn claim<'s>(&'s self, scope: &Scope<'s>, path: PathBuf, rules: Vec<RuleMatch>) {
        let entry = match Entry::lstat(self.gate, &path) {
            Ok(entry) => entry,
            Err(NotAnEntry::Placeholder { path }) => {
                return self.log.skip(path, WalkSkip::Placeholder);
            }
            Err(NotAnEntry::Fs(error)) => return self.log.entry_failed(path, error),
        };

        match self.behind_a_mount(&entry) {
            Ok(true) => self.log.skip(path, WalkSkip::Mount),
            Ok(false) => self.targets.claim(scope, entry, rules),
            Err(error) => self.log.problem(Problem::Path(error)),
        }
    }

    /// Whether the entry is a mount root, or lies inside a mount the walk may not
    /// enter.
    fn behind_a_mount(&self, entry: &Entry) -> Result<bool, fs::Error> {
        if entry.is_mount(self.gate)? {
            return Ok(true);
        }
        entry.is_behind_a_closed_mount(self.gate, self.home, self.mounts)
    }
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
