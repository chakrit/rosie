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
use super::entry::{Entry, InFolder, Readable, Sealed, Volume};
use super::log::{Log, Problem};
use super::progress::Progress;
use super::targets::Targets;
use crate::config::Walk;
use crate::fs::{Backend, FileKind, Gate, Metadata};
use crate::plan::{Needs, WalkSetting, WalkSkip};
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
    /// One Lua state per worker thread, started on the thread's first name-matched
    /// candidate.
    static LUA: RefCell<Option<LuaSandbox>> = const { RefCell::new(None) };
}

pub(super) struct TreeWalk<'a, B: Backend, P: Progress> {
    pub gate: &'a Gate<B>,
    pub rules: &'a RuleSet,
    /// The fixed rule paths at or below the start folder, by path.
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
    /// order. [`InFolder::of`] first tells a symlink, which is passed over, and a
    /// sealed entry, a placeholder or a mount root, which is never a target. Then the
    /// checks that need only the entry's `lstat`, its folder's, and its name: whether it
    /// is a placeholder the walk may not open, a mount root the walk may not enter (told
    /// from the two `lstat`s), and a bundle the walk may not enter.
    ///
    /// A closed placeholder or closed mount root ends there, and nothing reads it: a
    /// fixed path is matched without reading it, and otherwise a folder is one `Closed`
    /// skip carrying every closed setting, the bundle's included, so the settings that
    /// open it are known from that one skip.
    ///
    /// Only then do the rules look at the entry. A match is final: the walk never
    /// descends into a matched entry, and a matched sealed entry is a sealed skip (see
    /// `matched_step`). Bundles are matched but not entered: an unmatched closed bundle
    /// is a `Closed` skip that needs only `enter_bundles`.
    fn step<'p>(
        &self,
        path: &'p Path,
        name: &OsStr,
        meta: Metadata,
        siblings: &[OsString],
        folder_meta: Metadata,
    ) -> Step<'p> {
        let claim = match InFolder::of(path, meta, folder_meta) {
            InFolder::Symlink => return Step::Pass,
            InFolder::Sealed(_, sealed) => Err(sealed),
            InFolder::Entry(entry) => Ok(entry),
        };

        let is_folder = meta.kind == FileKind::Dir;
        let opened_placeholder = is_folder && self.flags.enter_placeholders;
        let closed_placeholder = meta.is_dataless() && !opened_placeholder;
        let closed_bundle = is_folder && is_bundle(path) && !self.flags.enter_bundles;
        let bundle = closed_bundle.then_some(WalkSetting::Bundles);
        let volume = Volume::of(path, meta, folder_meta, self.flags.into());
        let readable = match (volume, closed_placeholder) {
            (Volume::Readable(readable), false) => readable,
            (Volume::Readable(_), true) => {
                let needs = Needs::PLACEHOLDERS.and(bundle);
                return self.unread_step(path, claim, is_folder, needs);
            }
            (Volume::ClosedMount, _) => {
                let placeholder = closed_placeholder.then_some(WalkSetting::Placeholders);
                let needs = Needs::MOUNTS.and(placeholder).and(bundle);
                return self.unread_step(path, claim, is_folder, needs);
            }
        };

        let rules = self.matching(&readable, name, is_folder, siblings);

        match (rules.is_empty(), is_folder, closed_bundle) {
            (false, _, _) => matched_step(claim, rules),
            (true, false, _) => Step::Pass,
            (true, true, true) => Step::Skip(WalkSkip::Closed(Needs::BUNDLES)),
            (true, true, false) => Step::Descend(readable),
        }
    }

    /// An entry nothing may read: a closed placeholder, a closed mount root, or both, as
    /// `needs` records. Only a fixed path is known to match it (see `matched_step`). Any
    /// other folder is left closed, since opening it could reveal targets; a file is
    /// passed over.
    fn unread_step<'p>(
        &self,
        path: &Path,
        claim: Result<Entry, Sealed>,
        is_folder: bool,
        needs: Needs,
    ) -> Step<'p> {
        match (self.fixed.get(path), is_folder) {
            (Some(rules), _) => matched_step(claim, rules.clone()),
            (None, true) => Step::Skip(WalkSkip::Closed(needs)),
            (None, false) => Step::Pass,
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

/// A matched entry is claimed, unless it is sealed: a placeholder or a mount root is
/// never a target, so it is a `SealedPlaceholder` or `SealedMount` skip as
/// [`InFolder::of`] tells it.
fn matched_step<'p>(claim: Result<Entry, Sealed>, rules: Vec<RuleMatch>) -> Step<'p> {
    match claim {
        Ok(entry) => Step::Claim(entry, rules),
        Err(sealed) => Step::Skip(sealed.skip()),
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
