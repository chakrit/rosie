//! The parallel `tree` walk (`docs/spec/performance.md#scan`,
//! `docs/spec/safety.md#walk-skips`).
//!
//! Every folder is listed on its own rayon task, so idle workers steal folders. An entry
//! the listing reports as a file, a special file, or a link the walk does not follow is
//! passed over on the listing alone, unless a fixed rule path is in its folder. Every
//! other entry is `lstat`ed through the context before anything else happens to it, and
//! a folder is only ever listed after its own `lstat` said it is a folder, not a
//! symlink: every path the walk builds is free of links by construction.
//!
//! With `enter_links = "all"`, a symlink to a folder inside the roots is followed
//! (`docs/spec/safety.md#enter_links`): its target is looked up at its own real path,
//! as if the walk had found it there, so the paths the walk builds stay free of links.
//! Links can bring the walk to one folder twice, so then it keeps the real paths it has
//! entered or claimed, and goes no further on a second arrival.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::OsStr;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rayon::Scope;

use super::RuleMatch;
use super::entry::{Entry, InFolder, Readable, Sealed, Volume};
use super::fixed::FixedPaths;
use super::log::{Log, Problem};
use super::progress::Progress;
use super::skip::{Needs, WalkSkip};
use super::targets::Targets;
use crate::config::Walk;
use crate::config::WalkSetting;
use crate::fs::{
    self, Backend, Cleanup, DirEntry, EnterLinks, FileKind, FsContext, FsOp, Metadata,
};
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
    pub ctx: &'a FsContext<B>,
    pub rules: &'a RuleSet,
    /// The fixed rule paths at or below the start folder, by path.
    pub fixed: &'a FixedPaths,
    pub flags: Walk,
    pub targets: &'a Targets<'a, B, P>,
    pub log: &'a Log,
    pub walked: Walked,
}

/// The real paths the walk has entered or claimed. Kept only when links can bring the
/// walk to one path twice; otherwise every path is new by construction.
pub(super) enum Walked {
    Untracked,
    Tracked(Mutex<HashSet<PathBuf>>),
}

impl Walked {
    /// Tracking for a walk under `links`, with the start folder already entered.
    pub fn starting_at(root: &Path, links: EnterLinks) -> Walked {
        match links.follows_in_walk() {
            true => Walked::Tracked(Mutex::new(HashSet::from([root.to_path_buf()]))),
            false => Walked::Untracked,
        }
    }

    /// Records `path` as entered or claimed; `false` when it already was.
    fn first_time(&self, path: &Path) -> bool {
        match self {
            Walked::Untracked => true,
            Walked::Tracked(seen) => seen
                .lock()
                .expect("no walk worker panics while recording a folder")
                .insert(path.to_path_buf()),
        }
    }
}

/// What the walk does with one entry.
enum Step<'p> {
    Pass,
    Skip(WalkSkip),
    /// A skip named by another path than the entry's, such as a link's target.
    SkipAt(PathBuf, WalkSkip),
    Failed(fs::Error),
    Claim(Entry, Vec<RuleMatch>),
    Descend(Readable<'p>),
    /// A symlink the walk follows to the folder at `path`.
    Follow(PathBuf, Metadata),
}

impl<B: Backend + Sync, P: Progress> TreeWalk<'_, B, P> {
    /// Walks the start folder `root`, whose own `lstat` gave `root_meta`. The folder
    /// itself is never a target; one that is a dataless placeholder is not opened unless
    /// the walk may open placeholders.
    pub fn start<'s>(&'s self, scope: &Scope<'s>, root: PathBuf, root_meta: Metadata) {
        match root_meta.is_dataless() && !self.flags.enter_placeholders {
            true => self.log.skip(root, WalkSkip::Closed(Needs::PLACEHOLDERS)),
            false => self.visit(scope, root, root_meta),
        }
    }

    /// Walks from `folder`, whose own `lstat` gave `folder_meta`, down toward `target`,
    /// a path inside it. Each folder on the way is listed and its next entry judged
    /// among its siblings exactly as [`TreeWalk::visit`] would judge it, but only that
    /// entry: the walk goes on toward `target` past a folder it would descend into, and
    /// any other step on the way, such as a match or a skip, is taken as the walk takes
    /// it and ends the way down. `target` itself is taken as any entry is, so a folder
    /// the walk descends into is walked whole.
    pub fn visit_toward<'s>(
        &'s self,
        scope: &Scope<'s>,
        folder: PathBuf,
        folder_meta: Metadata,
        target: &Path,
    ) {
        let Some(name) = next_name(&folder, target) else {
            return;
        };
        let listed = match Cleanup::read_dir(&folder).perform(self.ctx) {
            Ok(listed) => listed,
            Err(error) => return self.log.entry_failed(folder, error),
        };
        let path = folder.join(name);
        let meta = match Cleanup::lstat(&path).perform(self.ctx) {
            Ok(meta) => meta,
            Err(error) => return self.log.entry_failed(path, error),
        };

        let step = self.step(&path, name, meta, &listed, folder_meta);
        match (path == target, step) {
            (false, Step::Descend(readable)) => {
                let inside = readable.path().to_path_buf();
                if self.walked.first_time(&inside) {
                    self.visit_toward(scope, inside, meta, target);
                }
            }
            (_, step) => self.take(scope, &path, meta, step, None),
        }
    }

    /// Lists `folder`, whose own `lstat` gave `folder_meta`, and handles each entry in
    /// it. Each entry's volume is compared with this folder's, so an entry inside a
    /// mount the walk entered is not taken for the root of one.
    pub fn visit<'s>(&'s self, scope: &Scope<'s>, folder: PathBuf, folder_meta: Metadata) {
        let listed = match Cleanup::read_dir(&folder).perform(self.ctx) {
            Ok(listed) => listed,
            Err(error) => return self.log.entry_failed(folder, error),
        };
        let holds_fixed = self.fixed.in_folder(&folder);

        let looked_at = listed
            .iter()
            .filter(|entry| holds_fixed || !self.passes_over(entry.kind));
        for entry in looked_at {
            let path = folder.join(&entry.name);
            let meta = match Cleanup::lstat(&path).perform(self.ctx) {
                Ok(meta) => meta,
                Err(error) => {
                    self.log.entry_failed(path, error);
                    continue;
                }
            };

            let step = self.step(&path, &entry.name, meta, &listed, folder_meta);
            self.take(scope, &path, meta, step, None);
        }
    }

    /// Whether an entry the listing reports as `kind` is passed over without an
    /// `lstat`: a file or special file, which no folder rule matches, or a link the walk
    /// does not follow. Only a fixed path could match one, so a folder holding a fixed
    /// path has every entry looked at.
    fn passes_over(&self, kind: FileKind) -> bool {
        match kind {
            FileKind::File | FileKind::Other => true,
            FileKind::Symlink => !self.flags.enter_links.follows_in_walk(),
            FileKind::Dir => false,
        }
    }

    /// Acts on the step for the entry at `path`, whose `lstat` gave `meta`. `link` is
    /// the symlink the walk followed to reach it, if any: arriving at an entry already
    /// entered or claimed is a skip of that link, and nothing on a plain arrival, which
    /// only a link could have beaten.
    fn take<'s>(
        &'s self,
        scope: &Scope<'s>,
        path: &Path,
        meta: Metadata,
        step: Step,
        link: Option<&Path>,
    ) {
        match step {
            Step::Pass => {}
            Step::Skip(reason) => self.log.skip(path.to_path_buf(), reason),
            Step::SkipAt(at, reason) => self.log.skip_named(path.to_path_buf(), at, reason),
            Step::Failed(error) => self.log.entry_failed(path.to_path_buf(), error),
            Step::Claim(entry, rules) => match self.walked.first_time(entry.path()) {
                true => self.targets.claim(scope, entry, rules),
                false => self.arrived_again(link),
            },
            Step::Descend(readable) => {
                let inside = readable.path().to_path_buf();
                match self.walked.first_time(&inside) {
                    true => scope.spawn(move |scope| self.visit(scope, inside, meta)),
                    false => self.arrived_again(link),
                }
            }
            Step::Follow(target, target_meta) => self.follow(scope, path, target, target_meta),
        }
    }

    fn arrived_again(&self, link: Option<&Path>) {
        if let Some(link) = link {
            self.log.skip(link.to_path_buf(), WalkSkip::LinkWalked);
        }
    }

    /// Looks at the folder a followed `link` leads to as if the walk had found it at
    /// its own `target` path: matched against the rules beside its own siblings, and
    /// entered on the same terms as any entry of its folder. `/` has no folder and no
    /// name for a rule to match, so a link to it, with `/` among the roots, is entered.
    fn follow<'s>(&'s self, scope: &Scope<'s>, link: &Path, target: PathBuf, meta: Metadata) {
        let (Some(folder), Some(name)) = (target.parent(), target.file_name()) else {
            return match self.walked.first_time(&target) {
                true => scope.spawn(move |scope| self.visit(scope, target, meta)),
                false => self.arrived_again(Some(link)),
            };
        };
        let folder_meta = match Cleanup::lstat(folder).perform(self.ctx) {
            Ok(folder_meta) => folder_meta,
            Err(error) => return self.log.entry_failed(folder.to_path_buf(), error),
        };
        let siblings = match Cleanup::read_dir(folder).perform(self.ctx) {
            Ok(siblings) => siblings,
            Err(error) => return self.log.entry_failed(folder.to_path_buf(), error),
        };

        let step = self.step(&target, name, meta, &siblings, folder_meta);
        self.take(scope, &target, meta, step, Some(link));
    }

    /// What the walk does with the symlink at `path` in the folder `folder_meta`
    /// describes. Only with `enter_links = "all"` does it look past the link: a link to
    /// a folder inside the roots is followed, on the same volume or with
    /// `enter_mounts`; a link to a folder outside them is a skip naming that folder; a
    /// link to nothing, or in a loop of links, is a skip; a link to anything else is
    /// passed over (`docs/spec/safety.md#enter_links`).
    fn link_step<'p>(&self, path: &Path, folder_meta: Metadata) -> Step<'p> {
        if !self.flags.enter_links.follows_in_walk() {
            return Step::Pass;
        }
        let target = match Cleanup::follow(path).perform(self.ctx) {
            Ok(target) => target,
            Err(error) => return unfollowed(error),
        };
        if target.meta.kind != FileKind::Dir {
            return Step::Pass;
        }

        let inside = match self.ctx.within_roots(&target.path) {
            Ok(inside) => inside,
            Err(error) => return Step::Failed(error),
        };
        let crosses = target.meta.dev != folder_meta.dev && !self.flags.enter_mounts;
        match (inside, crosses) {
            (false, _) => Step::SkipAt(target.path, WalkSkip::LinkOutsideRoots),
            (true, true) => Step::Skip(WalkSkip::Closed(Needs::MOUNTS)),
            (true, false) => Step::Follow(target.path, target.meta),
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
        siblings: &[DirEntry],
        folder_meta: Metadata,
    ) -> Step<'p> {
        let claim = match InFolder::of(path, meta, folder_meta) {
            InFolder::Symlink => return self.link_step(path, folder_meta),
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
        siblings: &[DirEntry],
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
        siblings: &[DirEntry],
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
                .filter_map(|rule| match rule.matches(self.ctx, lua, candidate) {
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

/// The skip for a link the walk could not follow: one to nothing or in a loop of links.
/// Any other failure is reported as the link's own.
fn unfollowed<'p>(error: fs::Error) -> Step<'p> {
    match error {
        fs::Error::LinkDangling { .. } => Step::Skip(WalkSkip::LinkDangling),
        fs::Error::LinkLoop { .. } => Step::Skip(WalkSkip::LinkLoop),
        error => Step::Failed(error),
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

/// The name of the entry of `folder` on the way to `target`; none when `target` is not
/// inside `folder`.
fn next_name<'t>(folder: &Path, target: &'t Path) -> Option<&'t OsStr> {
    let rest = target.strip_prefix(folder).ok()?;
    rest.components()
        .next()
        .map(|component| component.as_os_str())
}

fn is_bundle(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| BUNDLE_EXTENSIONS.contains(&extension))
}
