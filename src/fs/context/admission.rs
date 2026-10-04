//! Deleting a cleanup target in two steps: the context first admits the item
//! ([`FsContext::admit_delete`]), then deletes what it admitted ([`FsContext::delete`]).
//! [`crate::fs::ops::Delete`] is the one way in: `verify` admits only, so a caller with
//! work to do before the delete, such as booting out the launch job whose plist it is,
//! decides whether the delete can happen at all before doing any of that work; `perform`
//! admits again and deletes.

use std::path::{Path, PathBuf};

use super::removal::Entering;
use super::{Acting, FsContext, resolve_dots};
use crate::fs::backend::{Backend, Metadata};
use crate::fs::error::{Error, Op};

/// A cleanup target the context has admitted for deletion: no process runs a program
/// from inside it, it lies within the roots, it is reached through no symlink (or,
/// acting as root, only through folders no one but root can change), and it is neither
/// a symlink nor the root of a mounted volume. Only [`FsContext::admit_delete`] builds
/// one, so no delete can skip a check.
#[derive(Debug)]
pub(in crate::fs) struct Admitted {
    path: PathBuf,
    entering: Entering,
}

impl<B: Backend> FsContext<B> {
    /// Admits a file, folder tree, or other entry for [`FsContext::delete`] as whoever
    /// the context acts as. Acting as root, every folder above it must be one no one but
    /// root can change (see `root_control`): a folder that fails is
    /// [`Error::NotRootControlled`], and one that cannot be inspected is
    /// [`Error::AboveUnchecked`].
    pub(in crate::fs) fn admit_delete(&self, path: &Path) -> Result<Admitted, Error> {
        self.admit_delete_as(path, self.acting)
    }

    /// Admits an entry as `acting` would delete it, whoever this context acts as.
    pub(in crate::fs) fn admit_delete_as(
        &self,
        path: &Path,
        acting: Acting,
    ) -> Result<Admitted, Error> {
        let entering = match acting {
            Acting::User => Entering::OpeningUsersFolders,
            Acting::Root => Entering::RootControlledOnly,
        };

        let path = resolve_dots(path)?;
        self.refuse_running(&path)?;
        let path = self.confine_to_roots(&path)?;
        let meta = self.lstat_item(&path, entering)?;
        self.admit_item(&path, meta)?;
        Ok(Admitted { path, entering })
    }

    /// Checks the folders above an already confined item and `lstat`s it.
    fn lstat_item(&self, path: &Path, entering: Entering) -> Result<Metadata, Error> {
        match entering {
            Entering::OpeningUsersFolders => self.lstat_real(path),
            Entering::RootControlledOnly => {
                self.require_root_controlled_above(path)?;
                self.io(Op::Lstat, path, self.backend.lstat(path))
            }
        }
    }
}

impl<B: Backend + Sync> FsContext<B> {
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
    pub(in crate::fs) fn delete(&self, item: Admitted) -> Result<(), Error> {
        let meta = self.lstat_item(&item.path, item.entering)?;
        self.remove_item_in_parallel(&item.path, meta, item.entering)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::tests::{ctx, ctx_as_root, removals};
    use crate::fs::ROOT_UID;
    use crate::fs::backend::Backend;
    use crate::fs::error::Error;
    use crate::fs::fake::FakeBackend;

    #[test]
    fn refuses_a_delete_whose_folder_became_a_symlink_after_admission() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/code/app/target/out", "x");
        fake.add_file("/Users/me/projects/target/keep", "x");
        let ctx = ctx(&fake, &["/Users/me/code"]);
        let admitted = ctx
            .admit_delete(Path::new("/Users/me/code/app/target"))
            .expect("admitted before the swap");

        let moved = Path::new("/Users/me/code/old");
        fake.rename(Path::new("/Users/me/code/app"), moved)
            .expect("move the folder away");
        fake.add_symlink("/Users/me/code/app", "/Users/me/projects");
        let result = ctx.delete(admitted);

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
        let user = ctx(&fake, &["/Users/me"]);
        let root = ctx_as_root(&fake, &["/Users/me"]);
        let as_user = user
            .admit_delete(Path::new(item))
            .expect("admitted as the user");
        let as_root = root
            .admit_delete(Path::new(item))
            .expect("admitted as root");

        fake.mount(item, 9);
        let results = [user.delete(as_user), root.delete(as_root)];

        for result in results {
            assert!(
                matches!(&result, Err(Error::CrossesVolume { path }) if path == Path::new(item)),
                "expected a volume refusal, got {result:?}"
            );
        }
        assert!(fake.exists(format!("{item}/data.bin")));
        assert_eq!(removals(&fake), vec![]);
    }

    #[test]
    fn refuses_a_delete_while_a_process_runs_a_program_from_inside_it() {
        let fake = FakeBackend::new();
        fake.add_file("/opt/homebrew/Cellar/tmux/3.7c/bin/tmux", "x");
        fake.respond_running(&[(33012, "/opt/homebrew/Cellar/tmux/3.7c/bin/tmux")]);
        let ctx = ctx(&fake, &["/opt/homebrew"]);

        let result = ctx.admit_delete(Path::new("/opt/homebrew/Cellar/tmux"));

        assert!(
            matches!(&result, Err(Error::Running { pid: 33012, program, .. })
                if program == Path::new("/opt/homebrew/Cellar/tmux/3.7c/bin/tmux")),
            "expected a running refusal, got {result:?}"
        );
    }

    #[test]
    fn refuses_every_delete_when_the_running_programs_cannot_be_read() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/code/app/target/out", "x");
        fake.respond(
            crate::fs::running_argv(),
            crate::fs::CommandOutput {
                exit: crate::fs::Exit::Code(1),
                stdout: Vec::new(),
                stderr: b"lsof: broken".to_vec(),
            },
        );
        let ctx = ctx(&fake, &["/Users/me/code"]);

        let result = ctx.admit_delete(Path::new("/Users/me/code/app/target"));

        assert!(
            matches!(&result, Err(Error::RunningUnknown { reason }) if reason.contains("lsof: broken")),
            "expected the running check to fail, got {result:?}"
        );
    }
}
