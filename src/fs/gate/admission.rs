//! Deleting a cleanup target in two steps: the gate first admits the item
//! ([`Gate::admit_delete`], [`Gate::admit_delete_as_root`]), then deletes what it
//! admitted ([`Gate::delete`]). A caller with work to do before the delete, such as
//! booting out the launch job whose plist it is, decides with the admission whether
//! the delete can happen at all before doing any of that work.

use std::path::{Path, PathBuf};

use super::Gate;
use super::removal::Entering;
use crate::fs::backend::{Backend, Metadata};
use crate::fs::error::{Error, Op};

/// A cleanup target the gate has admitted for deletion: it lies within the roots, it
/// is reached through no symlink (or, acting as root, only through folders no one but
/// root can change), and it is neither a symlink nor the root of a mounted volume. Only
/// the gate's admit methods build one, so no delete can skip the roots check.
#[derive(Debug)]
pub struct Admitted {
    path: PathBuf,
    entering: Entering,
}

impl<B: Backend> Gate<B> {
    /// Admits a file, folder tree, or other entry for [`Gate::delete`] as the user.
    pub fn admit_delete(&self, path: &Path) -> Result<Admitted, Error> {
        self.admit(path, Entering::OpeningUsersFolders)
    }

    /// Admits an entry for [`Gate::delete`] acting as root for the user: every folder
    /// above it must be one no one but root can change (see `root_control`). A folder
    /// that fails is [`Error::NotRootControlled`]; one that cannot be inspected is
    /// [`Error::AboveUnchecked`].
    pub fn admit_delete_as_root(&self, path: &Path) -> Result<Admitted, Error> {
        self.admit(path, Entering::RootControlledOnly)
    }

    fn admit(&self, path: &Path, entering: Entering) -> Result<Admitted, Error> {
        let path = self.confine_to_roots(path)?;
        let meta = self.lstat_item(&path, entering)?;
        self.admit_item(&path, meta)?;
        Ok(Admitted { path, entering })
    }

    /// Checks the folders above an already confined item and `lstat`s it.
    fn lstat_item(&self, path: &Path, entering: Entering) -> Result<Metadata, Error> {
        match entering {
            Entering::OpeningUsersFolders => self.lstat_without_symlinks(path),
            Entering::RootControlledOnly => {
                self.require_root_controlled_above(path)?;
                self.io(Op::Lstat, path, self.backend.lstat(path))
            }
        }
    }
}

impl<B: Backend + Sync> Gate<B> {
    /// Deletes an admitted item and everything below it, the entries of each folder in
    /// parallel (`docs/spec/performance.md#run`). The disk may have changed since the
    /// admission, so every check of it is made again before anything is touched.
    ///
    /// Symlinks inside a folder are removed as links, never descended. Acting as the
    /// user, user-owned folders that are not writable are made writable on the way
    /// down; acting as root, only folders no one but root can change are entered, and
    /// no permissions are changed, so a folder inside the item that fails is reported
    /// as [`Error::NotRootControlled`] before anything below it is touched. A dataless
    /// cloud placeholder is refused, never opened or unlinked, and so is an entry on
    /// another volume than its folder. The delete stops at the first failure and reports
    /// it; entries already removed, or being removed in parallel, stay removed.
    pub fn delete(&self, item: Admitted) -> Result<(), Error> {
        let meta = self.lstat_item(&item.path, item.entering)?;
        self.remove_item_in_parallel(&item.path, meta, item.entering)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::tests::{gate, removals};
    use crate::fs::ROOT_UID;
    use crate::fs::backend::Backend;
    use crate::fs::error::Error;
    use crate::fs::fake::FakeBackend;

    #[test]
    fn refuses_a_delete_whose_folder_became_a_symlink_after_admission() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/code/app/target/out", "x");
        fake.add_file("/Users/me/projects/target/keep", "x");
        let gate = gate(&fake, &["/Users/me/code"]);
        let admitted = gate
            .admit_delete(Path::new("/Users/me/code/app/target"))
            .expect("admitted before the swap");

        let moved = Path::new("/Users/me/code/old");
        fake.rename(Path::new("/Users/me/code/app"), moved)
            .expect("move the folder away");
        fake.add_symlink("/Users/me/code/app", "/Users/me/projects");
        let result = gate.delete(admitted);

        assert!(
            matches!(&result, Err(Error::Symlink { link, .. }) if link == Path::new("/Users/me/code/app")),
            "expected a symlink refusal, got {result:?}"
        );
        assert!(fake.exists("/Users/me/projects/target/keep"));
        assert_eq!(removals(&fake), vec![]);
    }

    #[test]
    fn refuses_a_delete_whose_item_became_a_mount_point_after_admission() {
        let item = "/Users/me/code/disk";
        let fake = FakeBackend::running_as(ROOT_UID);
        fake.add_file(format!("{item}/data.bin"), "x");
        fake.chown_with_ancestors(item, ROOT_UID);
        let gate = gate(&fake, &["/Users/me"]);
        let as_user = gate
            .admit_delete(Path::new(item))
            .expect("admitted as the user");
        let as_root = gate
            .admit_delete_as_root(Path::new(item))
            .expect("admitted as root");

        fake.mount(item, 9);
        let results = [gate.delete(as_user), gate.delete(as_root)];

        for result in results {
            assert!(
                matches!(&result, Err(Error::CrossesVolume { path }) if path == Path::new(item)),
                "expected a volume refusal, got {result:?}"
            );
        }
        assert!(fake.exists(format!("{item}/data.bin")));
        assert_eq!(removals(&fake), vec![]);
    }
}
