//! Fixed rule paths as scan targets (`docs/spec/rules.md#paths`).
//!
//! Each path is resolved with the injected home and then located on disk, so two
//! spellings of one path, such as `/Users/me/.npm` and `~/.npm`, are one target listing
//! both rules. A path through a symlink hits the symlink ban and is reported like any
//! refused path, unless `enter_links` allows the link; then the path is a target at its
//! real location (`docs/spec/safety.md#enter_links`). An absent path is simply not
//! planned.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use rayon::Scope;

use super::RuleMatch;
use super::entry::{Entry, Mounts, NotAnEntry};
use super::log::{Log, Problem};
use super::progress::Progress;
use super::skip::{Needs, WalkSkip};
use super::targets::Targets;
use crate::fs::{self, Backend, Cleanup, EnterLinks, FsContext, FsOp};

use crate::rules::{FixedPath, RuleId, RuleSet, Shape};
use crate::tier::Tier;

/// Every fixed rule path located on disk, with the rules that list it, and every path
/// that could not be located, as written, with why.
pub(super) struct FixedTargets {
    located: BTreeMap<PathBuf, Vec<RuleMatch>>,
    refused: Vec<(PathBuf, fs::Error)>,
}

impl FixedTargets {
    pub fn of<B: Backend>(rules: &RuleSet, ctx: &FsContext<B>, links: EnterLinks) -> Self {
        let mut written: BTreeMap<PathBuf, Vec<RuleMatch>> = BTreeMap::new();
        for (rule, tier, fixed) in fixed_paths(rules) {
            let matched = RuleMatch {
                rule: rule.clone(),
                tier,
            };
            add(&mut written, fixed.resolve(ctx.paths()), [matched]);
        }

        let mut located: BTreeMap<PathBuf, Vec<RuleMatch>> = BTreeMap::new();
        let mut refused = Vec::new();
        for (path, matches) in written {
            match Cleanup::locate(&path, links).perform(ctx) {
                Ok(real) => add(&mut located, real, matches),
                Err(error) => refused.push((path, error)),
            }
        }
        FixedTargets { located, refused }
    }

    /// Only the paths at or below `root`: located ones by where they are, refused ones
    /// as written.
    pub fn under(self, root: &Path) -> Self {
        let located = self
            .located
            .into_iter()
            .filter(|(path, _)| path.starts_with(root))
            .collect();
        let refused = self
            .refused
            .into_iter()
            .filter(|(path, _)| path.starts_with(root))
            .collect();
        FixedTargets { located, refused }
    }

    /// Only the paths on the way down to `path` and those at or below it: located ones
    /// by where they are, refused ones as written.
    pub fn along(self, path: &Path) -> Self {
        let on_the_way = |fixed: &Path| path.starts_with(fixed) || fixed.starts_with(path);
        let located = self
            .located
            .into_iter()
            .filter(|(fixed, _)| on_the_way(fixed))
            .collect();
        let refused = self
            .refused
            .into_iter()
            .filter(|(fixed, _)| on_the_way(fixed))
            .collect();
        FixedTargets { located, refused }
    }

    /// Only the outermost located paths: one inside another collapses into it, as the
    /// walk would never reach it past a match.
    pub fn outermost(self) -> Self {
        let mut kept: BTreeMap<PathBuf, Vec<RuleMatch>> = BTreeMap::new();
        for (path, rules) in self.located {
            // `Path` orders by component, so an enclosing path sorts just before.
            let enclosed = kept
                .last_key_value()
                .is_some_and(|(outer, _)| path.starts_with(outer));
            if !enclosed {
                kept.insert(path, rules);
            }
        }
        FixedTargets {
            located: kept,
            refused: self.refused,
        }
    }

    /// Reports each path that could not be located, such as one through a symlink
    /// `enter_links` does not allow, and returns the located ones. The walk never
    /// enters a link, so it would pass such a path by without a word.
    pub fn report_refused(self, log: &Log) -> BTreeMap<PathBuf, Vec<RuleMatch>> {
        for (path, error) in self.refused {
            log.entry_failed(path, error);
        }
        self.located
    }
}

/// Adds `matches` to the rules listing `path`, each rule once.
fn add(
    targets: &mut BTreeMap<PathBuf, Vec<RuleMatch>>,
    path: PathBuf,
    matches: impl IntoIterator<Item = RuleMatch>,
) {
    let listed = targets.entry(path).or_default();
    for matched in matches {
        if !listed.contains(&matched) {
            listed.push(matched);
        }
    }
}

/// The located paths, for the walk to look each entry up in.
pub(super) fn by_path(
    located: BTreeMap<PathBuf, Vec<RuleMatch>>,
) -> HashMap<PathBuf, Vec<RuleMatch>> {
    located.into_iter().collect()
}

/// Claims fixed paths for `caches`, checking each path itself as the walk would, and the
/// volumes of the folders on its way down; no folder above it is checked for being a
/// placeholder. A denied path is a walk skip and an absent one is not planned. A dataless
/// placeholder is never planned. A path on another volume than its folder is a mount
/// root and is never planned, even with `enter_mounts`: the context refuses to delete a
/// mount point at all. Without `enter_mounts`, a path inside a mount is not planned
/// either: one reached through a folder on another volume than the folder above it,
/// counting from home for a path under home and from `/` otherwise.
pub(super) struct FixedClaim<'a, B: Backend, P: Progress> {
    pub ctx: &'a FsContext<B>,
    pub mounts: Mounts,
    pub targets: &'a Targets<'a, B, P>,
    pub log: &'a Log,
}

impl<B: Backend + Sync, P: Progress> FixedClaim<'_, B, P> {
    pub fn claim<'s>(&'s self, scope: &Scope<'s>, path: PathBuf, rules: Vec<RuleMatch>) {
        let entry = match Entry::lstat(self.ctx, &path) {
            Ok(entry) => entry,
            Err(NotAnEntry::Placeholder { path }) => {
                return self.log.skip(path, WalkSkip::SealedPlaceholder);
            }
            Err(NotAnEntry::Mount { path }) => return self.log.skip(path, WalkSkip::SealedMount),
            Err(NotAnEntry::Fs(error)) => return self.log.entry_failed(path, error),
        };

        let closed = entry.is_behind_a_closed_mount(self.ctx, self.mounts);
        match closed {
            Ok(false) => self.targets.claim(scope, entry, rules),
            Ok(true) => self.log.skip(path, WalkSkip::Closed(Needs::MOUNTS)),
            Err(error) => self.log.problem(Problem::Path {
                met_at: path,
                error,
            }),
        }
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
