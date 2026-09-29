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
use super::entry::Entry;
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
    /// The start folder's volume; entries on any other are mounts.
    pub root_dev: u64,
    pub targets: &'a Targets<'a, B, P>,
    pub log: &'a Log,
}

/// What the walk does with one entry.
enum Step {
    Pass,
    Skip(WalkSkip),
    Claim(Entry, Vec<RuleMatch>),
    Descend,
}

impl<B: Backend + Sync, P: Progress> TreeWalk<'_, B, P> {
    /// Lists `folder`, already `lstat`ed as a folder, and handles each entry in it.
    pub fn visit<'s>(&'s self, scope: &Scope<'s>, folder: PathBuf) {
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

            match self.step(&path, name, meta, &names) {
                Step::Pass => {}
                Step::Skip(reason) => self.log.skip(path, reason),
                Step::Claim(entry, rules) => self.targets.claim(scope, entry, rules),
                Step::Descend => scope.spawn(move |scope| self.visit(scope, path)),
            }
        }
    }

    /// Symlinks are passed over. Mounts and placeholders are skipped before any rule
    /// looks at them, so neither is ever a target unless its flag lets the walk in; a
    /// placeholder is never a target at all. A match stops the descent; bundles are
    /// matched but not entered.
    fn step(&self, path: &Path, name: &OsStr, meta: Metadata, siblings: &[OsString]) -> Step {
        if meta.kind == FileKind::Symlink {
            return Step::Pass;
        }
        if meta.dev != self.root_dev && !self.flags.enter_mounts {
            return Step::Skip(WalkSkip::Mount);
        }

        let is_folder = meta.kind == FileKind::Dir;
        let Some(entry) = Entry::walked(path, meta) else {
            let enterable = is_folder && self.flags.enter_placeholders;
            let wanted = is_folder || self.fixed.contains_key(path);
            return match (enterable, wanted) {
                (true, _) => Step::Descend,
                (false, true) => Step::Skip(WalkSkip::Placeholder),
                (false, false) => Step::Pass,
            };
        };

        let rules = self.matching(path, name, is_folder, siblings);
        if !rules.is_empty() {
            return Step::Claim(entry, rules);
        }

        let closed_bundle = is_bundle(path) && !self.flags.enter_bundles;
        match (is_folder, closed_bundle) {
            (false, _) => Step::Pass,
            (true, true) => Step::Skip(WalkSkip::Bundle),
            (true, false) => Step::Descend,
        }
    }

    // matching

    /// The fixed paths naming `path`, then the folder rules accepting it.
    fn matching(
        &self,
        path: &Path,
        name: &OsStr,
        is_folder: bool,
        siblings: &[OsString],
    ) -> Vec<RuleMatch> {
        let fixed = self.fixed.get(path).into_iter().flatten().cloned();
        let folder = match is_folder {
            true => self.folder_matches(path, name, siblings),
            false => Vec::new(),
        };
        fixed.chain(folder).collect()
    }

    /// The folder rules whose target is `name`, the folder's own name, and whose
    /// detection accepts it. Lua runs only for these name-matched candidates.
    fn folder_matches(&self, path: &Path, name: &OsStr, siblings: &[OsString]) -> Vec<RuleMatch> {
        let named = self.rules.targeting(name);
        if named.is_empty() {
            return Vec::new();
        }

        let candidate = Candidate { path, siblings };
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
