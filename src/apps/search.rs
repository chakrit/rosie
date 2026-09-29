//! Collects what an app or orphan scan found into a plan: each item gated by `roots`,
//! marked `sudo` when the user does not own it, a launch plist carrying its bootout,
//! and sized by the scanner's sizer once the search is done (`docs/spec/app.md`,
//! `docs/spec/plan.md`).
//!
//! No item is sized across into another volume: the sizer never leaves the volume of
//! the item it starts from, and a matched entry that is itself on another volume than
//! its leftover location is a mount, skipped rather than planned. A matched dataless
//! placeholder is skipped too, never planned or opened.

use std::path::PathBuf;

use super::listing::{Listed, find_folder, list_found};
use super::locations::{Holds, Location};
use super::{Error, Scanned};
use crate::fs::{Backend, Gate, Home};
use crate::plan::{
    AggressiveItems, ItemKind, LaunchDomain, PathMatch, PlanBuilder, Reach, Report, RunAs, Twin,
    WalkSkip,
};
use crate::scan::{Entry, InFolder, Log, Mounts, NoProgress, Sizer};

/// A file or folder a scan may match by name. Symlinks never match, so none is ever a
/// candidate.
pub(super) struct Candidate {
    /// The entry's name; names that are not UTF-8 match nothing and are left out.
    pub name: String,
    found: Found,
}

/// What a matched candidate becomes.
enum Found {
    Item {
        entry: Entry,
        kind: ItemKind,
    },
    /// A walk skip: the scan does not enter it, so it is never planned.
    Skip {
        path: PathBuf,
        reason: WalkSkip,
    },
}

impl Candidate {
    /// The candidate for a file or folder; nothing for a special file.
    pub(super) fn of(entry: Entry, name: String) -> Option<Candidate> {
        let kind = entry.item_kind()?;
        let found = Found::Item { entry, kind };
        Some(Candidate { name, found })
    }

    fn skipped(path: PathBuf, name: String, reason: WalkSkip) -> Candidate {
        let found = Found::Skip { path, reason };
        Candidate { name, found }
    }
}

/// How a matched item is planned, apart from its size.
struct Marks {
    kind: ItemKind,
    launch: Option<LaunchDomain>,
    reach: Reach,
    run_as: RunAs,
    rule: String,
    twin: Twin,
}

pub(super) struct Search<'g, B: Backend> {
    gate: &'g Gate<B>,
    /// Home anchors the volume check of each leftover location.
    home: &'g Home,
    mounts: Mounts,
    builder: PlanBuilder,
    matched: Vec<(Entry, Marks)>,
    log: Log,
}

impl<'g, B: Backend + Sync> Search<'g, B> {
    pub(super) fn new(
        gate: &'g Gate<B>,
        home: &'g Home,
        mounts: Mounts,
        aggressive: AggressiveItems,
    ) -> Self {
        Search {
            gate,
            home,
            mounts,
            builder: PlanBuilder::new(aggressive),
            matched: Vec::new(),
            log: Log::default(),
        }
    }

    /// The candidates in a leftover location. Symlinks never match, so they are left
    /// out; a location rosie may not list is a walk skip, and an entry that vanishes
    /// between listing and `lstat` is gone. A location reached from home, or from `/`,
    /// across another volume is a `Mount` skip without `enter_mounts`, as a fixed rule
    /// path is in `caches`, and is not listed.
    pub(super) fn candidates(&mut self, location: &Location) -> Result<Vec<Candidate>, Error> {
        let Some(folder) = find_folder(self.gate, &location.path)? else {
            return Ok(Vec::new());
        };
        if folder.is_behind_a_closed_mount(self.gate, self.home, self.mounts)? {
            self.log.skip(location.path.clone(), WalkSkip::Mount);
            return Ok(Vec::new());
        }

        let names = match list_found(self.gate, &folder)? {
            Listed::Names(names) => names,
            Listed::Denied(_) => {
                self.log.skip(location.path.clone(), WalkSkip::Denied);
                return Ok(Vec::new());
            }
        };

        let mut candidates = Vec::new();
        for os_name in names {
            let Some(name) = os_name.to_str().map(str::to_owned) else {
                continue;
            };
            let found = match folder.entry(self.gate, &os_name) {
                Ok(found) => found,
                Err(error) if error.has_vanished() => continue,
                Err(error) => return Err(error.into()),
            };
            match found {
                InFolder::Entry(entry) => candidates.extend(Candidate::of(entry, name)),
                InFolder::Symlink(_) => {}
                InFolder::Mount(path) => {
                    candidates.push(Candidate::skipped(path, name, WalkSkip::Mount));
                }
                InFolder::Placeholder(path) => {
                    candidates.push(Candidate::skipped(path, name, WalkSkip::Placeholder));
                }
            }
        }
        Ok(candidates)
    }

    /// Adds a matched candidate found in a location holding `holds`.
    pub(super) fn add(
        &mut self,
        candidate: Candidate,
        holds: Holds,
        rule: &str,
        twin: Twin,
    ) -> Result<(), Error> {
        let (entry, kind) = match candidate.found {
            Found::Item { entry, kind } => (entry, kind),
            Found::Skip { path, reason } => {
                self.log.skip(path, reason);
                return Ok(());
            }
        };
        let user_uid = self.gate.user_uid();

        let reach = match self.gate.within_roots(entry.path())? {
            true => Reach::InRoots,
            false => Reach::OutsideRoots,
        };
        let run_as = match entry.meta().uid == user_uid {
            true => RunAs::User,
            false => RunAs::Sudo,
        };
        let launch = match kind {
            ItemKind::File => holds.launch_domain(user_uid),
            ItemKind::Folder => None,
        };

        let marks = Marks {
            kind,
            launch,
            reach,
            run_as,
            rule: rule.to_owned(),
            twin,
        };
        self.matched.push((entry, marks));
        Ok(())
    }

    pub(super) fn add_receipt(&mut self, package: &str, twin: Twin) -> Result<(), Error> {
        self.builder.add_receipt(package, twin)?;
        Ok(())
    }

    pub(super) fn add_report(&mut self, report: Report) {
        self.builder.add_report(report);
    }

    /// Sizes every matched item in parallel and builds the plan. An error sizing an
    /// item, other than a walk skip, fails the scan.
    pub(super) fn finish(self) -> Result<Scanned, Error> {
        let Search {
            gate,
            home: _,
            mounts: _,
            mut builder,
            matched,
            log,
        } = self;

        let sized = {
            let sizer = Sizer::walking(gate, &NoProgress, &log);
            rayon::scope(|scope| {
                let started = matched.into_iter().map(|(entry, marks)| {
                    let path = entry.path().to_path_buf();
                    (path, marks, sizer.measure(scope, entry))
                });
                started.collect::<Vec<_>>()
            })
        };
        let (mut skipped, problems) = log.into_parts();
        if let Some(problem) = problems.into_iter().next() {
            return Err(Error::Sizing(problem));
        }

        for (path, marks, size) in sized {
            let found = PathMatch {
                path,
                kind: marks.kind,
                size: size.get(),
                run_as: marks.run_as,
                reach: marks.reach,
                rule: marks.rule,
                twin: marks.twin,
            };
            match marks.launch {
                Some(domain) => builder.add_launch_job(found, domain)?,
                None => builder.add_path(found)?,
            }
        }
        skipped.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Scanned {
            plan: builder.build(),
            skipped,
        })
    }
}
