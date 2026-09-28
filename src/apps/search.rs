//! Collects what an app or orphan scan found into a plan: each item sized, gated by
//! `roots`, marked `sudo` when the user does not own it, and a launch plist carrying its
//! bootout (`docs/spec/app.md`, `docs/spec/plan.md`).

use std::path::PathBuf;

use super::listing::{Listing, list_folder};
use super::locations::{Holds, Location};
use super::{Error, Scanned};
use crate::fs::{Backend, FileKind, Gate, Metadata};
use crate::plan::{
    AggressiveItems, ItemKind, PathMatch, PlanBuilder, Reach, Report, RunAs, Twin, WalkSkip,
    WalkSkips,
};
use crate::sizing::Sizer;

/// A file or folder a scan may list. Symlinks never match, so none is ever an entry.
pub(super) struct Entry {
    pub path: PathBuf,
    /// The entry's name; names that are not UTF-8 match nothing and are left out.
    pub name: String,
    meta: Metadata,
    kind: ItemKind,
}

impl Entry {
    /// The entry for a file or folder; nothing for a symlink or a special file.
    pub(super) fn of(path: PathBuf, name: String, meta: Metadata) -> Option<Entry> {
        let kind = match meta.kind {
            FileKind::File => ItemKind::File,
            FileKind::Dir => ItemKind::Folder,
            FileKind::Symlink | FileKind::Other => return None,
        };
        Some(Entry {
            path,
            name,
            meta,
            kind,
        })
    }
}

pub(super) struct Search<'g, B: Backend> {
    gate: &'g Gate<B>,
    builder: PlanBuilder,
    sizer: Sizer,
    skips: WalkSkips,
}

impl<'g, B: Backend> Search<'g, B> {
    pub(super) fn new(gate: &'g Gate<B>, aggressive: AggressiveItems) -> Self {
        Search {
            gate,
            builder: PlanBuilder::new(aggressive),
            sizer: Sizer::new(),
            skips: WalkSkips::default(),
        }
    }

    /// The files and folders in a leftover location. Symlinks never match, so they are
    /// left out; a location rosie may not list is a walk skip, and an entry that vanishes
    /// between listing and `lstat` is gone.
    pub(super) fn entries(&mut self, location: &Location) -> Result<Vec<Entry>, Error> {
        let names = match list_folder(self.gate, &location.path)? {
            Listing::Entries(names) => names,
            Listing::NoFolder => return Ok(Vec::new()),
            Listing::Denied(_) => {
                self.skips.record(WalkSkip::Denied);
                return Ok(Vec::new());
            }
        };

        let mut entries = Vec::new();
        for name in names {
            let path = location.path.join(&name);
            let meta = match self.gate.lstat(&path) {
                Ok(meta) => meta,
                Err(error) if error.has_vanished() => continue,
                Err(error) => return Err(error.into()),
            };
            let Some(name) = name.to_str() else {
                continue;
            };
            entries.extend(Entry::of(path, name.to_owned(), meta));
        }
        Ok(entries)
    }

    /// Adds a matched file or folder found in a location holding `holds`.
    pub(super) fn add(
        &mut self,
        entry: Entry,
        holds: Holds,
        rule: &str,
        twin: Twin,
    ) -> Result<(), Error> {
        let kind = entry.kind;
        let user_uid = self.gate.user_uid();

        let size = self
            .sizer
            .measure(self.gate, &entry.path, entry.meta, &mut self.skips)?;
        let reach = match self.gate.within_roots(&entry.path)? {
            true => Reach::InRoots,
            false => Reach::OutsideRoots,
        };
        let run_as = match entry.meta.uid == user_uid {
            true => RunAs::User,
            false => RunAs::Sudo,
        };
        let found = PathMatch {
            path: entry.path,
            kind,
            size,
            run_as,
            reach,
            rule: rule.to_owned(),
            twin,
        };

        match (kind, holds.launch_domain(user_uid)) {
            (ItemKind::File, Some(domain)) => self.builder.add_launch_job(found, domain)?,
            _ => self.builder.add_path(found)?,
        }
        Ok(())
    }

    pub(super) fn add_receipt(&mut self, package: &str, twin: Twin) -> Result<(), Error> {
        self.builder.add_receipt(package, twin)?;
        Ok(())
    }

    pub(super) fn add_report(&mut self, report: Report) {
        self.builder.add_report(report);
    }

    pub(super) fn finish(self) -> Scanned {
        Scanned {
            plan: self.builder.build(),
            skips: self.skips,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(kind: FileKind) -> Metadata {
        Metadata {
            kind,
            allocated: 4096,
            dev: 1,
            inode: 2,
            uid: 501,
            mode: 0o644,
            flags: 0,
        }
    }

    /// Sockets, FIFOs, and device nodes are neither a file nor a folder a plan can hold,
    /// so they are left out like symlinks.
    #[test]
    fn only_files_and_folders_are_entries() {
        let kind_of = |kind| {
            let path = PathBuf::from("/Users/me/Library/Caches/com.foo.Bar");
            Entry::of(path, "com.foo.Bar".to_owned(), meta(kind)).map(|entry| entry.kind)
        };

        assert_eq!(kind_of(FileKind::File), Some(ItemKind::File));
        assert_eq!(kind_of(FileKind::Dir), Some(ItemKind::Folder));
        assert_eq!(kind_of(FileKind::Symlink), None);
        assert_eq!(kind_of(FileKind::Other), None);
    }
}
