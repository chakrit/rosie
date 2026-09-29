//! The parallel `tree` walk (`docs/spec/performance.md#scan`,
//! `docs/spec/safety.md#walk-skips`).
//!
//! Every folder is listed on its own rayon task, so idle workers steal folders. Each
//! entry is `lstat`ed through the gate before anything else happens to it, and a
//! folder is only ever listed after its own `lstat` said it is a folder, not a
//! symlink: every path the walk builds is free of links by construction.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use rayon::Scope;

use super::RuleMatch;
use super::entry::{Entry, Readable, Volume};
use super::log::{Log, Problem};
use super::progress::Progress;
use super::targets::Targets;
use crate::config::Walk;
use crate::fs::{Backend, FileKind, Gate, Metadata};
use crate::plan::WalkSkip;
use crate::rules::{self, Candidate, LuaSandbox, RuleSet};

/// Folder extensions of macOS bundles, which the walk does not enter by default.
const BUNDLE_EXTENSIONS: [&str; 8] = [
    "app",
    "framework",
    "bundle",
    "plugin",
    "xcarchive",
    "photoslibrary",
    "sparsebundle",
    "dSYM",
];

thread_local! {
    /// One Lua state per worker thread, started on the thread's first Lua candidate.
    static LUA: RefCell<Option<LuaSandbox>> = const { RefCell::new(None) };
}

pub(super) struct TreeWalk<'a, B: Backend, P: Progress> {
    pub gate: &'a Gate<B>,
    pub rules: &'a RuleSet,
    /// The fixed rule paths below the start folder, by path.
    pub fixed: &'a HashMap<PathBuf, Vec<RuleMatch>>,
    pub flags: Walk,
    pub targets: &'a Targets<'a, B, P>,
    pub log: &'a Log,
}

/// What the walk does with one entry.
enum Step<'p> {
    Pass,
    Skip(WalkSkip),
    Claim(Entry, Vec<RuleMatch>),
    Descend(Readable<'p>),
}

impl<B: Backend + Sync, P: Progress> TreeWalk<'_, B, P> {
    /// Lists `folder`, whose own `lstat` gave `folder_meta`, and handles each entry in
    /// it. Each entry's volume is compared with this folder's, so an entry inside a
    /// mount the walk entered is not taken for the root of one.
    pub fn visit<'s>(&'s self, scope: &Scope<'s>, folder: PathBuf, folder_meta: Metadata) {
        let names = match self.gate.read_dir(&folder) {
            Ok(names) => names,
            Err(error) => return self.log.entry_failed(folder, error),
        };

        for name in &names {
            let path = folder.join(name);
            let meta = match self.gate.lstat(&path) {
                Ok(meta) => meta,
                Err(error) => {
                    self.log.entry_failed(path, error);
                    continue;
                }
            };

            match self.step(&path, name, meta, &names, folder_meta) {
                Step::Pass => {}
                Step::Skip(reason) => self.log.skip(path, reason),
                Step::Claim(entry, rules) => self.targets.claim(scope, entry, rules),
                Step::Descend(readable) => {
                    let inside = readable.path().to_path_buf();
                    scope.spawn(move |scope| self.visit(scope, inside, meta));
                }
            }
        }
    }

    /// What the walk does with an entry of the folder `folder_meta` describes, in
    /// order. Symlinks are passed over. A placeholder is skipped before any rule looks
    /// at it, so it is never a target. Then the entry's volume is told from the two
    /// `lstat`s: a mount root the walk may not enter ends there, as a `Bundle` skip when
    /// it is a closed bundle and a `Mount` skip when it is a folder or a fixed path, and
    /// nothing reads inside it.
    /// Only then do the rules look at the entry. A match stops the descent; a match on a
    /// mount root is a `Mount` skip, since the gate refuses to delete a mount point.
    /// Bundles are matched but not entered.
    fn step<'p>(
        &self,
        path: &'p Path,
        name: &OsStr,
        meta: Metadata,
        siblings: &[OsString],
        folder_meta: Metadata,
    ) -> Step<'p> {
        if meta.kind == FileKind::Symlink {
            return Step::Pass;
        }

        let is_folder = meta.kind == FileKind::Dir;
        let volume = Volume::of(path, meta, folder_meta, self.flags.into());
        let Some(entry) = Entry::walked(path, meta) else {
            return self.placeholder_step(path, is_folder, volume);
        };
        let closed_bundle = is_folder && is_bundle(path) && !self.flags.enter_bundles;
        let wanted = is_folder || self.fixed.contains_key(path);
        let readable = match (volume, closed_bundle, wanted) {
            (Volume::Readable(readable), _, _) => readable,
            (Volume::ClosedMount, true, _) => return Step::Skip(WalkSkip::Bundle),
            (Volume::ClosedMount, false, true) => return Step::Skip(WalkSkip::Mount),
            (Volume::ClosedMount, false, false) => return Step::Pass,
        };

        let rules = self.matching(&readable, name, is_folder, siblings);
        let matched = !rules.is_empty();

        match (matched, readable.is_mount_root(), is_folder, closed_bundle) {
            (true, true, _, _) => Step::Skip(WalkSkip::Mount),
            (true, false, _, _) => Step::Claim(entry, rules),
            (false, _, false, _) => Step::Pass,
            (false, _, true, true) => Step::Skip(WalkSkip::Bundle),
            (false, _, true, false) => Step::Descend(readable),
        }
    }

    /// A dataless placeholder is opened only with `enter_placeholders`, and a mounted
    /// one only where the walk may also enter the mount.
    fn placeholder_step<'p>(&self, path: &Path, is_folder: bool, volume: Volume<'p>) -> Step<'p> {
        let enterable = is_folder && self.flags.enter_placeholders;
        let wanted = is_folder || self.fixed.contains_key(path);
        match (enterable, wanted, volume) {
            (true, _, Volume::Readable(readable)) => Step::Descend(readable),
            (true, _, Volume::ClosedMount) => Step::Skip(WalkSkip::Mount),
            (false, true, _) => Step::Skip(WalkSkip::Placeholder),
            (false, false, _) => Step::Pass,
        }
    }

    // matching

    /// The fixed paths naming the entry, then the folder rules accepting it.
    fn matching(
        &self,
        entry: &Readable,
        name: &OsStr,
        is_folder: bool,
        siblings: &[OsString],
    ) -> Vec<RuleMatch> {
        let fixed = self.fixed.get(entry.path()).into_iter().flatten().cloned();
        let folder = match is_folder {
            true => self.folder_matches(entry, name, siblings),
            false => Vec::new(),
        };
        fixed.chain(folder).collect()
    }

    /// The folder rules whose target is `name`, the folder's own name, and whose
    /// detection accepts it. Lua runs only for these name-matched candidates.
    fn folder_matches(
        &self,
        entry: &Readable,
        name: &OsStr,
        siblings: &[OsString],
    ) -> Vec<RuleMatch> {
        let named = self.rules.targeting(name);
        if named.is_empty() {
            return Vec::new();
        }

        let candidate = Candidate {
            path: entry.path(),
            siblings,
        };
        let checked = with_lua(|lua| {
            named
                .iter()
                .filter_map(|rule| match rule.matches(self.gate, lua, candidate) {
                    Ok(true) => Some(RuleMatch {
                        rule: rule.id().clone(),
                        tier: rule.tier(),
                    }),
                    Ok(false) => None,
                    Err(error) => {
                        self.log.problem(Problem::Rule(error));
                        None
                    }
                })
                .collect()
        });
        checked.unwrap_or_else(|error| {
            self.log.problem(Problem::Rule(error));
            Vec::new()
        })
    }
}

/// Runs `check` with this thread's Lua state, starting it on first use.
fn with_lua<T>(check: impl FnOnce(&LuaSandbox) -> T) -> Result<T, rules::Error> {
    LUA.with(|slot| {
        let started = slot.take();
        let sandbox = match started {
            Some(sandbox) => sandbox,
            None => LuaSandbox::new()?,
        };

        let checked = check(&sandbox);
        slot.replace(Some(sandbox));
        Ok(checked)
    })
}

fn is_bundle(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| BUNDLE_EXTENSIONS.contains(&extension))
}
