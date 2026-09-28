//! `rename(2)` as the macOS kernel runs it, checked in the kernel's order.
//!
//! The kernel names four parties: the source entry and its folder, and the entry the new
//! name already resolves to (if any) and the folder that name goes in. "Same file" is
//! decided by inode, not by name, and permission is checked before anything is found to
//! be a no-op. The permission rules follow XNU's rename authorization, quirks included:
//! replacing a folder checks write permission on the replaced folder, not its parent.
//! `tests/contract_rename.rs` pins each rule against the machine.

use std::ffi::OsStr;
use std::io::{self, ErrorKind};
use std::path::{Component, Path, PathBuf};

use super::{Creatable, FinalLink, Location, State, Tail, parent_of};

/// The entries one rename touches.
struct Parties {
    source: PathBuf,
    source_folder: PathBuf,
    /// The existing entry the new name resolves to, which may be the source itself.
    replaced: Option<PathBuf>,
    /// Where the source ends up: the replaced entry's stored name, or the new name.
    destination: PathBuf,
    destination_folder: PathBuf,
}

impl State {
    pub(super) fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        let parties = self.rename_parties(from, to)?;

        if Tail::of(from).is_dot_name() || Tail::of(to).is_dot_name() {
            return Err(ErrorKind::InvalidInput.into());
        }
        self.require_same_volume(&parties)?;
        self.require_matching_kinds(&parties)?;
        if parties.destination_folder.starts_with(&parties.source) {
            // Into its own subtree.
            return Err(ErrorKind::InvalidInput.into());
        }
        self.authorize_rename(&parties)?;

        let replaced = match &parties.replaced {
            None => None,
            Some(entry) if self.entries[entry] == self.entries[&parties.source] => {
                return self.rename_same_file(&parties, to);
            }
            Some(entry) => Some(entry),
        };
        if let Some(entry) = replaced {
            if self.is_dir(entry) && self.children(entry).next().is_some() {
                return Err(ErrorKind::DirectoryNotEmpty.into());
            }
            self.entries.remove(entry);
        }

        self.rekey_subtree(&parties.source, &parties.destination);
        Ok(())
    }

    // steps

    /// Looks both paths up. The source is the entry itself, a final link included,
    /// unless the path demands a folder.
    fn rename_parties(&self, from: &Path, to: &Path) -> io::Result<Parties> {
        let source = self.locate(from, FinalLink::Keep)?.existing()?;
        let source_is_dir = self.is_dir(&source);
        let (replaced, destination) = match self.locate(to, FinalLink::Keep)? {
            Location::Found(entry) => (Some(entry.clone()), entry),
            Location::Absent(_, Creatable::FolderOnly) if !source_is_dir => {
                return Err(ErrorKind::NotFound.into());
            }
            Location::Absent(name, _) => (None, name),
        };

        Ok(Parties {
            source_folder: parent_of(&source)?.to_path_buf(),
            destination_folder: parent_of(&destination)?.to_path_buf(),
            source,
            replaced,
            destination,
        })
    }

    fn require_same_volume(&self, parties: &Parties) -> io::Result<()> {
        let source_dev = self.node_at(&parties.source).dev;
        let destination_dev = self.node_at(&parties.destination_folder).dev;
        match source_dev == destination_dev {
            true => Ok(()),
            false => Err(ErrorKind::CrossesDevices.into()),
        }
    }

    /// A folder replaces only a folder, and a non-folder only a non-folder.
    fn require_matching_kinds(&self, parties: &Parties) -> io::Result<()> {
        let Some(replaced) = &parties.replaced else {
            return Ok(());
        };
        match (self.is_dir(&parties.source), self.is_dir(replaced)) {
            (true, false) => Err(ErrorKind::NotADirectory.into()),
            (false, true) => Err(ErrorKind::IsADirectory.into()),
            _ => Ok(()),
        }
    }

    /// XNU's rename authorization. The old name is always removed from the source
    /// folder. The source "moves" when it changes folder, or when it replaces a folder
    /// other than its own parent. A moving folder has its `..` rewritten, which needs
    /// write permission on it. The add is checked on the replaced folder if there is
    /// one (write alone), else on the destination folder.
    fn authorize_rename(&self, parties: &Parties) -> io::Result<()> {
        let source_folder = parties.source_folder.as_path();
        let replaced_folder = parties
            .replaced
            .as_deref()
            .filter(|entry| self.is_dir(entry));
        let replaces_non_folder = parties.replaced.is_some() && replaced_folder.is_none();
        let moves = match replaced_folder {
            Some(folder) => folder != source_folder,
            None => parties.destination_folder != source_folder,
        };

        self.require_writable_dir(source_folder)?;
        if moves && self.is_dir(&parties.source) {
            self.require_write_bit(&parties.source)?;
        }
        match (moves, replaced_folder) {
            (true, Some(folder)) => self.require_write_bit(folder)?,
            (true, None) => self.require_writable_dir(&parties.destination_folder)?,
            (false, _) => {}
        }
        if replaces_non_folder {
            self.require_writable_dir(&parties.destination_folder)?;
        }
        Ok(())
    }

    /// The new name resolves to the source's own inode. A case-only respelling of the
    /// same entry takes the typed spelling; anything else, such as one hardlink onto
    /// another, changes nothing.
    fn rename_same_file(&mut self, parties: &Parties, to: &Path) -> io::Result<()> {
        let Some(typed_name) = last_name(to) else {
            return Ok(());
        };
        let same_entry = parties.replaced.as_ref() == Some(&parties.source);

        if same_entry && Some(typed_name) != parties.source.file_name() {
            let respelling = parties.source_folder.join(typed_name);
            self.rekey_subtree(&parties.source, &respelling);
        }
        Ok(())
    }

    /// Moves an entry and everything below it to a new stored path.
    fn rekey_subtree(&mut self, from: &Path, to: &Path) {
        let moved: Vec<(PathBuf, PathBuf, u64)> = self
            .subtree(from)
            .filter_map(|(old, inode)| {
                let below = old.strip_prefix(from).ok()?;
                let new = match below.as_os_str().is_empty() {
                    true => to.to_path_buf(),
                    false => to.join(below),
                };
                Some((old.clone(), new, inode))
            })
            .collect();

        for (old, _, _) in &moved {
            self.entries.remove(old);
        }
        for (_, new, inode) in moved {
            self.entries.insert(new, inode);
        }
    }
}

/// The last plain name of a typed path, ignoring any trailing `/`.
fn last_name(path: &Path) -> Option<&OsStr> {
    match path.components().next_back() {
        Some(Component::Normal(name)) => Some(name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::path::Path;

    use crate::fs::backend::Backend;
    use crate::fs::fake::FakeBackend;

    #[test]
    fn refuses_to_move_across_volumes() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/file", "x");
        fake.add_dir("/Volumes/ext");
        fake.mount("/Volumes/ext", 7);

        let moved = fake.rename(Path::new("/Users/me/file"), Path::new("/Volumes/ext/file"));

        assert_eq!(moved.map_err(|e| e.kind()), Err(ErrorKind::CrossesDevices));
        assert!(fake.exists("/Users/me/file"));
    }
}
