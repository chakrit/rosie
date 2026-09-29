//! `Gate<B>`: the one wrapper every other part of rosie receives instead of a backend.
//!
//! - Cleanup-target mutations are refused outside the injected `roots` and through any
//!   symlink (`docs/spec/safety.md#cleanup-targets`, `#symlinks`).
//! - Rosie's own data goes through separate methods confined to the injected config and
//!   data folders, not checked against roots (`docs/spec/safety.md#rosies-own-data`).
//! - Reads and commands pass through, with errors naming their path or command.

mod components;
mod lexical;
mod removal;
mod root_control;

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use super::backend::{Argv, Backend, CommandOutput, Exit, Metadata};
use super::error::{Error, Op};
use removal::Entering;

pub use lexical::resolve_dots;
pub use root_control::Exposure;

/// Everything the gate is bounded by, injected by the caller and never read from the
/// environment.
#[derive(Debug, Clone)]
pub struct Bounds {
    /// Folders that cleanup-target mutations must lie within.
    pub roots: Vec<PathBuf>,
    /// Rosie's config folder, `~/.config/rosie`.
    pub config_dir: PathBuf,
    /// Rosie's data folder, `~/.local/share/rosie`.
    pub data_dir: PathBuf,
    /// The user rosie acts for; only their read-only folders are made writable to delete.
    pub user_uid: u32,
}

pub struct Gate<B: Backend> {
    backend: B,
    roots: Vec<PathBuf>,
    own_dirs: [PathBuf; 2],
    user_uid: u32,
}

impl<B: Backend> Gate<B> {
    pub fn new(backend: B, bounds: Bounds) -> Result<Self, Error> {
        let roots = bounds
            .roots
            .iter()
            .map(|root| resolve_dots(root))
            .collect::<Result<Vec<_>, _>>()?;
        let config_dir = resolve_dots(&bounds.config_dir)?;
        let data_dir = resolve_dots(&bounds.data_dir)?;

        Ok(Gate {
            backend,
            roots,
            own_dirs: [config_dir, data_dir],
            user_uid: bounds.user_uid,
        })
    }

    // reads

    pub fn read_dir(&self, path: &Path) -> Result<Vec<OsString>, Error> {
        let path = resolve_dots(path)?;
        self.io(Op::ReadDir, &path, self.backend.read_dir(&path))
    }

    pub fn lstat(&self, path: &Path) -> Result<Metadata, Error> {
        let path = resolve_dots(path)?;
        self.io(Op::Lstat, &path, self.backend.lstat(&path))
    }

    /// `lstat`s a path of a scanned tree, such as a file a Lua rule tests for or a fixed
    /// leftover folder app mode lists. A path with a symlink in any component, the last
    /// included, is refused rather than followed; a missing component is an `Io` error
    /// naming it.
    pub fn lstat_link_free(&self, path: &Path) -> Result<Metadata, Error> {
        let path = resolve_dots(path)?;
        self.lstat_without_symlinks(&path)
    }

    /// Reads a file of a scanned tree, such as a marker a Lua rule inspects. A path with
    /// a symlink in any component is refused rather than followed.
    pub fn read_file(&self, path: &Path) -> Result<Vec<u8>, Error> {
        let path = resolve_dots(path)?;
        self.lstat_without_symlinks(&path)?;
        self.io(Op::ReadFile, &path, self.backend.read_file(&path))
    }

    // commands

    /// Runs a tool command. Commands are outside the roots gate; the plan lists each for
    /// confirmation (`docs/spec/safety.md#cleanup-targets`).
    pub fn run(&self, argv: &Argv) -> Result<CommandOutput, Error> {
        self.backend.run(argv).map_err(|source| Error::Command {
            argv: argv.clone(),
            source,
        })
    }

    /// Runs a tool command like [`Gate::run`], handing each line of its output to
    /// `on_line` as it arrives.
    pub fn run_streaming<F: FnMut(&str)>(&self, argv: &Argv, on_line: F) -> Result<Exit, Error> {
        self.backend
            .run_streaming(argv, on_line)
            .map_err(|source| Error::Command {
                argv: argv.clone(),
                source,
            })
    }

    /// Runs `sudo <argv>` with plain `sudo`, piping `stdin` to it
    /// (`docs/spec/safety.md#elevation`).
    pub fn sudo(&self, argv: &Argv, stdin: &[u8]) -> Result<CommandOutput, Error> {
        self.backend
            .sudo(argv, stdin)
            .map_err(|source| Error::Command {
                argv: argv.clone(),
                source,
            })
    }

    // rosie's own data

    // Every own-data path is refused through a symlink in any component, the own
    // folders and their ancestors included (`docs/spec/safety.md#symlinks`).

    /// Reads a file under rosie's own folders.
    pub fn read_own_file(&self, path: &Path) -> Result<Vec<u8>, Error> {
        let path = self.confine_to_own(path)?;
        self.lstat_without_symlinks(&path)?;
        self.io(Op::ReadFile, &path, self.backend.read_file(&path))
    }

    /// Creates or replaces a file under rosie's own folders.
    pub fn write_own_file(&self, path: &Path, contents: &[u8]) -> Result<(), Error> {
        let path = self.confine_to_own(path)?;
        self.refuse_symlinks_in_existing(&path)?;
        self.io(
            Op::WriteFile,
            &path,
            self.backend.write_file(&path, contents),
        )
    }

    /// Creates a new file under rosie's own folders. Fails with `AlreadyExists` when
    /// the volume already holds the name under any spelling it treats as the same.
    pub fn create_own_file(&self, path: &Path, contents: &[u8]) -> Result<(), Error> {
        let path = self.confine_to_own(path)?;
        self.refuse_symlinks_in_existing(&path)?;
        self.io(
            Op::CreateFile,
            &path,
            self.backend.create_file(&path, contents),
        )
    }

    /// Creates a folder under rosie's own folders, with any missing parents.
    pub fn create_own_dir_all(&self, path: &Path) -> Result<(), Error> {
        let path = self.confine_to_own(path)?;
        self.refuse_symlinks_in_existing(&path)?;
        self.io(Op::CreateDir, &path, self.backend.create_dir_all(&path))
    }

    /// Moves an entry within rosie's own folders, such as a freshly unpacked pack into
    /// its final place. The own folders themselves are never moved or replaced.
    pub fn rename_own(&self, from: &Path, to: &Path) -> Result<(), Error> {
        let from = self.confine_below_own(from)?;
        let to = self.confine_below_own(to)?;
        self.lstat_without_symlinks(&from)?;
        self.refuse_symlinks_in_existing(&to)?;
        self.io(Op::Rename, &from, self.backend.rename(&from, &to))
    }

    /// Deletes an entry inside rosie's own folders, such as a removed pack. The own
    /// folders themselves are never deleted.
    pub fn delete_own(&self, path: &Path) -> Result<(), Error> {
        let path = self.confine_below_own(path)?;
        let meta = self.lstat_without_symlinks(&path)?;
        self.remove_item(&path, meta)
    }

    // bounds

    /// Whether a cleanup-target mutation of `path` passes the roots check: `.` and `..`
    /// resolved by text, then compared component by component with every root
    /// (`docs/spec/safety.md#cleanup-targets`). The disk is not consulted.
    pub fn within_roots(&self, path: &Path) -> Result<bool, Error> {
        let path = resolve_dots(path)?;
        Ok(self.contains(&path))
    }

    /// The user rosie acts for.
    pub fn user_uid(&self) -> u32 {
        self.user_uid
    }

    fn confine_to_roots(&self, path: &Path) -> Result<PathBuf, Error> {
        let path = resolve_dots(path)?;
        match self.contains(&path) {
            true => Ok(path),
            false => Err(Error::OutsideRoots { path }),
        }
    }

    /// Whether an already resolved path lies within a root.
    fn contains(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root))
    }

    fn confine_to_own(&self, path: &Path) -> Result<PathBuf, Error> {
        let path = resolve_dots(path)?;
        match self.own_dirs.iter().any(|dir| path.starts_with(dir)) {
            true => Ok(path),
            false => Err(Error::OutsideOwnData { path }),
        }
    }

    /// Confines to an entry strictly inside one of rosie's own folders.
    fn confine_below_own(&self, path: &Path) -> Result<PathBuf, Error> {
        let path = self.confine_to_own(path)?;
        match self.own_dirs.contains(&path) {
            true => Err(Error::OwnFolderItself { path }),
            false => Ok(path),
        }
    }

    /// Attaches the operation and path to a backend error.
    fn io<T>(&self, op: Op, path: &Path, result: io::Result<T>) -> Result<T, Error> {
        result.map_err(|source| Error::Io {
            op,
            path: path.to_path_buf(),
            source,
        })
    }
}

impl<B: Backend + Sync> Gate<B> {
    // cleanup targets

    /// Deletes a file, folder tree, or other entry that lies within the roots, the
    /// entries of each folder in parallel (`docs/spec/performance.md#run`).
    ///
    /// Symlinks inside a folder are removed as links, never descended. User-owned
    /// folders that are not writable are made writable on the way down. A dataless
    /// cloud placeholder is refused, never opened or unlinked, and so is an entry on
    /// another volume than its folder, the item itself included. The delete stops at a
    /// failure and reports it; entries removed before it stay removed.
    pub fn delete(&self, path: &Path) -> Result<(), Error> {
        let path = self.confine_to_roots(path)?;
        let meta = self.lstat_without_symlinks(&path)?;
        self.remove_item_in_parallel(&path, meta, Entering::OpeningUsersFolders)
    }

    /// Deletes like [`Gate::delete`], acting as root for the user: only through
    /// folders no one but root can change (see `root_control`), and without changing
    /// any permissions.
    ///
    /// A folder above the item, the item, or a folder inside it that fails is reported
    /// as [`Error::NotRootControlled`] before anything below it is touched; entries
    /// removed before it stay removed. A folder above the item that cannot be
    /// inspected is [`Error::AboveUnchecked`].
    pub fn delete_as_root(&self, path: &Path) -> Result<(), Error> {
        let path = self.confine_to_roots(path)?;
        self.require_root_controlled_above(&path)?;
        let meta = self.io(Op::Lstat, &path, self.backend.lstat(&path))?;
        self.remove_item_in_parallel(&path, meta, Entering::RootControlledOnly)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::io::ErrorKind;

    use super::*;
    use crate::fs::ROOT_UID;
    use crate::fs::error::RealPath;
    use crate::fs::fake::{Call, FakeBackend, USER_UID};

    const HOME: &str = "/Users/me";
    const CONFIG: &str = "/Users/me/.config/rosie";
    const DATA: &str = "/Users/me/.local/share/rosie";

    fn gate<'a>(fake: &'a FakeBackend, roots: &[&str]) -> Gate<&'a FakeBackend> {
        let bounds = Bounds {
            roots: roots.iter().map(PathBuf::from).collect(),
            config_dir: PathBuf::from(CONFIG),
            data_dir: PathBuf::from(DATA),
            user_uid: USER_UID,
        };
        Gate::new(fake, bounds).expect("absolute bounds")
    }

    fn removals(fake: &FakeBackend) -> Vec<Call> {
        fake.calls()
            .into_iter()
            .filter(|call| matches!(call, Call::RemoveFile(_) | Call::RemoveEmptyDir(_)))
            .collect()
    }

    fn path(text: &str) -> &Path {
        Path::new(text)
    }

    // roots

    #[test]
    fn deletes_a_tree_within_a_root() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/code/app/node_modules/a/index.js", "x");
        fake.add_file("/Users/me/code/app/node_modules/b.js", "x");
        fake.add_file("/Users/me/code/app/package.json", "{}");
        let gate = gate(&fake, &["/Users/me/code"]);

        gate.delete(path("/Users/me/code/app/node_modules"))
            .expect("delete within root");

        assert!(!fake.exists("/Users/me/code/app/node_modules"));
        assert!(fake.exists("/Users/me/code/app/package.json"));
    }

    #[test]
    fn deletes_a_root_itself() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/.npm/_cacache/index", "x");
        let gate = gate(&fake, &["/Users/me/.npm/_cacache"]);

        gate.delete(path("/Users/me/.npm/_cacache"))
            .expect("a root is within itself");

        assert!(!fake.exists("/Users/me/.npm/_cacache"));
    }

    #[test]
    fn refuses_a_path_outside_every_root() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/Documents/thesis.tex", "x");
        let gate = gate(&fake, &["/Users/me/code", "/Users/me/Library/Caches"]);

        let result = gate.delete(path("/Users/me/Documents"));

        assert!(
            matches!(result, Err(Error::OutsideRoots { path }) if path == Path::new("/Users/me/Documents"))
        );
        assert!(fake.exists("/Users/me/Documents/thesis.tex"));
        assert_eq!(removals(&fake), vec![]);
    }

    #[test]
    fn resolves_parent_components_before_the_roots_check() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/code");
        fake.add_file("/Users/me/Documents/thesis.tex", "x");
        let gate = gate(&fake, &["/Users/me/code"]);

        let escaped = gate.delete(path("/Users/me/code/../Documents"));

        assert!(
            matches!(escaped, Err(Error::OutsideRoots { path }) if path == Path::new("/Users/me/Documents"))
        );
        assert!(fake.exists("/Users/me/Documents/thesis.tex"));
    }

    #[test]
    fn resolves_current_and_parent_components_that_stay_inside() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/code/app/target/debug/app", "x");
        let gate = gate(&fake, &["/Users/me/code"]);

        gate.delete(path("/Users/me/code/./app/src/../target"))
            .expect("stays inside root");

        assert!(!fake.exists("/Users/me/code/app/target"));
    }

    #[test]
    fn compares_roots_by_component_not_by_string_prefix() {
        let fake = FakeBackend::new();
        fake.add_file("/foobar/keep.txt", "x");
        let gate = gate(&fake, &["/foo"]);

        let result = gate.delete(path("/foobar/keep.txt"));

        assert!(matches!(result, Err(Error::OutsideRoots { .. })));
        assert!(fake.exists("/foobar/keep.txt"));
    }

    #[test]
    fn answers_whether_a_path_lies_within_the_roots_as_delete_decides_it() {
        let fake = FakeBackend::new();
        let gate = gate(&fake, &["/Users/me/code", "/Users/me/Library/Caches"]);

        let within = |text| gate.within_roots(path(text)).expect("absolute path");

        assert!(within("/Users/me/code"));
        assert!(within("/Users/me/code/app/node_modules"));
        assert!(within("/Users/me/Library/Caches/pip"));
        assert!(!within("/Users/me/code2"));
        assert!(!within("/Users/me/code/../Documents"));
        assert!(matches!(
            gate.within_roots(path("code")),
            Err(Error::Relative { .. })
        ));
    }

    #[test]
    fn refuses_relative_paths() {
        let fake = FakeBackend::new();
        let gate = gate(&fake, &["/Users/me/code"]);

        let result = gate.delete(path("code/app"));

        assert!(matches!(result, Err(Error::Relative { .. })));
    }

    // symlinks

    #[test]
    fn refuses_to_delete_through_a_symlink_component_naming_the_real_path() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/projects/app/target/out", "x");
        fake.add_symlink("/Users/me/code/escape", "../projects");
        let gate = gate(&fake, &["/Users/me/code"]);

        let result = gate.delete(path("/Users/me/code/escape/app/target"));

        let Err(Error::Symlink { path, link, real }) = result else {
            panic!("expected a symlink refusal, got {result:?}");
        };
        assert_eq!(path, PathBuf::from("/Users/me/code/escape/app/target"));
        assert_eq!(link, PathBuf::from("/Users/me/code/escape"));
        assert!(
            matches!(real, RealPath::Resolved(real) if real == Path::new("/Users/me/projects/app/target"))
        );
        assert!(fake.exists("/Users/me/projects/app/target/out"));
        assert_eq!(removals(&fake), vec![]);
    }

    #[test]
    fn refuses_to_delete_a_symlink_given_as_the_target() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/elsewhere");
        fake.add_symlink("/Users/me/code/link", "/Users/me/elsewhere");
        let gate = gate(&fake, &["/Users/me/code"]);

        let result = gate.delete(path("/Users/me/code/link"));

        assert!(matches!(result, Err(Error::Symlink { .. })));
        assert!(fake.exists("/Users/me/code/link"));
    }

    #[test]
    fn removes_symlinks_inside_a_deleted_folder_as_links() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/src/lib/keep.js", "x");
        fake.add_symlink("/Users/me/code/app/node_modules/lib", "/Users/me/src/lib");
        let gate = gate(&fake, &["/Users/me/code"]);

        gate.delete(path("/Users/me/code/app/node_modules"))
            .expect("delete folder");

        assert!(!fake.exists("/Users/me/code/app/node_modules"));
        assert!(fake.exists("/Users/me/src/lib/keep.js"));
        assert!(!fake.calls().contains(&Call::ReadDir(PathBuf::from(
            "/Users/me/code/app/node_modules/lib"
        ))));
    }

    #[test]
    fn refuses_to_read_a_symlinked_file() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/secret.toml", "x");
        fake.add_symlink("/Users/me/code/app/Cargo.toml", "/Users/me/secret.toml");
        let gate = gate(&fake, &[]);

        let result = gate.read_file(path("/Users/me/code/app/Cargo.toml"));

        assert!(
            matches!(result, Err(Error::Symlink { real: RealPath::Resolved(real), .. }) if real == Path::new("/Users/me/secret.toml"))
        );
    }

    #[test]
    fn refuses_to_read_a_file_through_a_symlinked_folder() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/other/Cargo.toml", "x");
        fake.add_symlink("/Users/me/code/app/linked", "/Users/me/other");
        let gate = gate(&fake, &[]);

        let result = gate.read_file(path("/Users/me/code/app/linked/Cargo.toml"));

        assert!(
            matches!(result, Err(Error::Symlink { link, .. }) if link == Path::new("/Users/me/code/app/linked"))
        );
    }

    #[test]
    fn refuses_to_lstat_through_or_at_a_symlink() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/other/Cargo.toml", "x");
        fake.add_symlink("/Users/me/code/app/linked", "/Users/me/other");
        let gate = gate(&fake, &[]);

        let through = gate.lstat_link_free(path("/Users/me/code/app/linked/Cargo.toml"));
        let at = gate.lstat_link_free(path("/Users/me/code/app/linked"));
        let plain = gate.lstat_link_free(path("/Users/me/other/Cargo.toml"));

        for result in [through, at] {
            assert!(
                matches!(&result, Err(Error::Symlink { link, .. }) if link == Path::new("/Users/me/code/app/linked")),
                "expected a symlink refusal, got {result:?}"
            );
        }
        assert!(matches!(plain, Ok(meta) if meta.kind == crate::fs::FileKind::File));
    }

    #[test]
    fn refuses_to_lstat_a_relative_path() {
        let fake = FakeBackend::new();
        let gate = gate(&fake, &[]);

        let result = gate.lstat_link_free(path("rel/x"));

        assert!(matches!(result, Err(Error::Relative { .. })));
    }

    #[test]
    fn names_the_fully_real_path_when_a_link_leads_through_another_link() {
        let fake = FakeBackend::new();
        fake.add_dir("/private/tmp");
        fake.add_symlink("/tmp", "private/tmp");
        fake.add_symlink("/Users/me/code/escape", "/tmp");
        let gate = gate(&fake, &["/Users/me/code"]);

        let deleted = gate.delete(path("/Users/me/code/escape/x"));
        let typed = gate.check_typed_path(path("/Users/me/code/escape/x"));

        for result in [deleted, typed.map(drop)] {
            assert!(
                matches!(&result, Err(Error::Symlink { real: RealPath::Resolved(real), .. }) if real == Path::new("/private/tmp/x")),
                "expected the fully real path, got {result:?}"
            );
        }
    }

    #[test]
    fn keeps_a_refusal_typed_when_the_link_cannot_be_read() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/elsewhere");
        fake.add_symlink("/Users/me/code/link", "/Users/me/elsewhere");
        fake.fail_on(
            "/Users/me/code/link",
            Op::ReadLink,
            ErrorKind::PermissionDenied,
        );
        let gate = gate(&fake, &["/Users/me/code"]);

        let result = gate.delete(path("/Users/me/code/link/x"));

        assert!(
            matches!(result, Err(Error::Symlink { link, real: RealPath::Unresolved(_), .. }) if link == Path::new("/Users/me/code/link"))
        );
    }

    #[test]
    fn leaves_the_real_path_unresolved_for_a_symlink_loop() {
        let fake = FakeBackend::new();
        fake.add_symlink("/Users/me/code/a", "b");
        fake.add_symlink("/Users/me/code/b", "a");
        let gate = gate(&fake, &["/Users/me/code"]);

        let result = gate.delete(path("/Users/me/code/a/x"));

        assert!(matches!(
            result,
            Err(Error::Symlink {
                real: RealPath::Unresolved(_),
                ..
            })
        ));
    }

    // deletion

    #[test]
    fn makes_user_owned_read_only_folders_writable_on_the_way_down() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/go/pkg/mod/x@v1/go.mod", "module x");
        fake.chmod("/Users/me/go/pkg/mod/x@v1", 0o555);
        let gate = gate(&fake, &["/Users/me/go/pkg/mod"]);

        gate.delete(path("/Users/me/go/pkg/mod/x@v1"))
            .expect("read-only module deleted");

        assert!(!fake.exists("/Users/me/go/pkg/mod/x@v1"));
    }

    #[test]
    fn leaves_the_permissions_of_other_users_folders_alone() {
        let fake = FakeBackend::new();
        fake.add_file("/Library/Caches/tool/blob", "x");
        fake.chown("/Library/Caches/tool", ROOT_UID);
        fake.chmod("/Library/Caches/tool", 0o555);
        let gate = gate(&fake, &["/Library/Caches"]);

        let result = gate.delete(path("/Library/Caches/tool"));

        let Err(Error::Io { op, path, source }) = result else {
            panic!("expected a permission failure, got {result:?}");
        };
        assert_eq!(
            (op, source.kind()),
            (Op::RemoveFile, ErrorKind::PermissionDenied)
        );
        assert_eq!(path, PathBuf::from("/Library/Caches/tool/blob"));
        assert!(
            !fake
                .calls()
                .iter()
                .any(|call| matches!(call, Call::SetMode(..)))
        );
    }

    #[test]
    fn stops_at_a_failure_partway_through_and_names_its_path() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/code/app/target/a", "x");
        fake.add_file("/Users/me/code/app/target/b", "x");
        fake.add_file("/Users/me/code/app/target/c", "x");
        fake.fail_on(
            "/Users/me/code/app/target/b",
            Op::RemoveFile,
            ErrorKind::ResourceBusy,
        );
        let gate = gate(&fake, &["/Users/me/code"]);

        let result = gate.delete(path("/Users/me/code/app/target"));

        assert!(
            matches!(result, Err(Error::Io { op: Op::RemoveFile, path, .. }) if path == Path::new("/Users/me/code/app/target/b"))
        );
        assert!(!fake.exists("/Users/me/code/app/target/a"));
        assert!(fake.exists("/Users/me/code/app/target/b"));
        assert!(fake.exists("/Users/me/code/app/target"));
    }

    #[test]
    fn refuses_to_descend_into_another_volume() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/code/app/build/mnt/disk.bin", "x");
        fake.mount("/Users/me/code/app/build/mnt", 9);
        let gate = gate(&fake, &["/Users/me/code"]);

        let result = gate.delete(path("/Users/me/code/app/build"));

        assert!(
            matches!(result, Err(Error::CrossesVolume { path }) if path == Path::new("/Users/me/code/app/build/mnt"))
        );
        assert!(fake.exists("/Users/me/code/app/build/mnt/disk.bin"));
    }

    // A volume root is never a deletable item: every entry on it shares its volume, so
    // the check made on each entry inside would let the whole volume be emptied. This
    // holds for every delete entry point, including rosie's own data.
    #[test]
    fn refuses_to_delete_a_mount_point_without_touching_its_volume() {
        let item = "/Users/me/code/disk";
        let fake = FakeBackend::running_as(ROOT_UID);
        fake.add_file(format!("{item}/data.bin"), "x");
        fake.chown_with_ancestors(item, ROOT_UID);
        fake.mount(item, 9);
        let gate = gate(&fake, &["/Users/me"]);

        let as_user = gate.delete(path(item));
        let as_root = gate.delete_as_root(path(item));

        for result in [as_user, as_root] {
            assert!(
                matches!(&result, Err(Error::CrossesVolume { path }) if path == Path::new(item)),
                "expected a volume refusal, got {result:?}"
            );
        }
        assert!(fake.exists(format!("{item}/data.bin")));
        assert!(!fake.calls().iter().any(|call| matches!(
            call,
            Call::ReadDir(_) | Call::RemoveFile(_) | Call::RemoveEmptyDir(_) | Call::SetMode(..)
        )));
    }

    #[test]
    fn refuses_to_delete_a_mounted_pack_without_touching_its_volume() {
        let own_item = "/Users/me/.local/share/rosie/packs/p";
        let fake = FakeBackend::new();
        fake.add_file(format!("{own_item}/data.bin"), "x");
        fake.mount(own_item, 9);
        let gate = gate(&fake, &[]);

        let result = gate.delete_own(path(own_item));

        assert!(
            matches!(&result, Err(Error::CrossesVolume { path }) if path == Path::new(own_item)),
            "expected a volume refusal, got {result:?}"
        );
        assert!(fake.exists(format!("{own_item}/data.bin")));
        assert!(!fake.calls().iter().any(|call| matches!(
            call,
            Call::ReadDir(_) | Call::RemoveFile(_) | Call::RemoveEmptyDir(_) | Call::SetMode(..)
        )));
    }

    #[test]
    fn refuses_dataless_placeholders_without_opening_or_unlinking_them() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/code/a/cloud-dir/x", "x");
        fake.add_file("/Users/me/code/b/cloud-file", "x");
        fake.make_dataless("/Users/me/code/a/cloud-dir");
        fake.make_dataless("/Users/me/code/b/cloud-file");
        let gate = gate(&fake, &["/Users/me/code"]);

        let folder = gate.delete(path("/Users/me/code/a"));
        let file = gate.delete(path("/Users/me/code/b/cloud-file"));

        assert!(
            matches!(folder, Err(Error::Placeholder { path }) if path == Path::new("/Users/me/code/a/cloud-dir"))
        );
        assert!(matches!(file, Err(Error::Placeholder { .. })));
        assert!(fake.exists("/Users/me/code/a/cloud-dir/x"));
        assert!(fake.exists("/Users/me/code/b/cloud-file"));
        assert!(
            !fake
                .calls()
                .contains(&Call::ReadDir(PathBuf::from("/Users/me/code/a/cloud-dir")))
        );
    }

    #[test]
    fn deletes_own_data_one_entry_at_a_time_with_the_same_refusals() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/.local/share/rosie/packs/p/cloud/x", "x");
        fake.make_dataless("/Users/me/.local/share/rosie/packs/p/cloud");
        let gate = gate(&fake, &[]);

        let result = gate.delete_own(path("/Users/me/.local/share/rosie/packs/p"));

        assert!(matches!(result, Err(Error::Placeholder { .. })));
        assert!(fake.exists("/Users/me/.local/share/rosie/packs/p/cloud/x"));
    }

    // typed paths

    #[test]
    fn accepts_a_typed_path_spelled_as_on_disk() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/code/app");
        let gate = gate(&fake, &[]);

        let checked = gate
            .check_typed_path(path("/Users/me/code/./app"))
            .expect("exact spelling");

        assert_eq!(checked, PathBuf::from("/Users/me/code/app"));
    }

    #[test]
    fn refuses_a_case_mismatch_naming_the_correct_spelling() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/Code/App");
        let gate = gate(&fake, &[]);

        let result = gate.check_typed_path(path("/users/me/code/App"));

        let Err(Error::Misspelled { typed, real }) = result else {
            panic!("expected a spelling refusal, got {result:?}");
        };
        assert_eq!(typed, PathBuf::from("/users/me/code/App"));
        assert_eq!(real, PathBuf::from("/Users/me/Code/App"));
    }

    #[test]
    fn refuses_a_typed_path_through_a_symlink_naming_the_real_path() {
        let fake = FakeBackend::new();
        fake.add_dir("/private/var/folders/x");
        fake.add_symlink("/var", "private/var");
        let gate = gate(&fake, &[]);

        let result = gate.check_typed_path(path("/var/folders/x"));

        assert!(
            matches!(result, Err(Error::Symlink { real: RealPath::Resolved(real), .. }) if real == Path::new("/private/var/folders/x"))
        );
    }

    #[test]
    fn reports_a_missing_typed_path_as_not_found() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me");
        let gate = gate(&fake, &[]);

        let result = gate.check_typed_path(path("/Users/me/nope"));

        assert!(
            matches!(result, Err(Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound)
        );
    }

    // stored names

    #[test]
    fn names_the_entry_a_lookup_folds_onto_as_it_is_stored() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/packs/Rosie");
        fake.add_dir("/Users/me/packs/other");
        let gate = gate(&fake, &[]);

        let stored = gate
            .stored_name(path("/Users/me/packs"), OsStr::new("rosie"))
            .expect("folded lookup");
        let exact = gate
            .stored_name(path("/Users/me/packs"), OsStr::new("other"))
            .expect("exact lookup");

        assert_eq!(stored, OsString::from("Rosie"));
        assert_eq!(exact, OsString::from("other"));
    }

    #[test]
    fn reports_a_missing_stored_name_as_not_found() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/packs/rosie");
        let gate = gate(&fake, &[]);

        let result = gate.stored_name(path("/Users/me/packs"), OsStr::new("rosy"));

        assert!(
            matches!(result, Err(Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound)
        );
    }

    #[test]
    fn refuses_a_stored_name_that_is_not_one_entry_name() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/packs/rosie");
        let gate = gate(&fake, &[]);

        for name in ["", ".", "..", "rosie/..", "rosie/", "/etc", "a/b"] {
            let result = gate.stored_name(path("/Users/me/packs"), OsStr::new(name));

            assert!(
                matches!(&result, Err(Error::NotAnEntryName { name: refused }) if refused == name),
                "{name:?}: got {result:?}"
            );
        }
    }

    // rosie's own data

    #[test]
    fn writes_own_data_without_any_roots() {
        let fake = FakeBackend::new();
        fake.add_dir(HOME);
        let gate = gate(&fake, &[]);
        let config = Path::new(CONFIG).join("config.toml");
        let pack = Path::new(DATA).join("packs/builtin");

        gate.create_own_dir_all(Path::new(CONFIG))
            .expect("create config folder");
        gate.write_own_file(&config, b"roots = []")
            .expect("write config");
        gate.create_own_dir_all(&pack).expect("create pack folder");
        gate.delete_own(&pack).expect("remove pack");

        assert_eq!(
            gate.read_own_file(&config).expect("read config"),
            b"roots = []"
        );
        assert!(!fake.exists(&pack));
    }

    #[test]
    fn confines_own_data_to_rosies_folders() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/.zshrc", "x");
        let gate = gate(&fake, &[HOME]);

        let outside = gate.write_own_file(path("/Users/me/.zshrc"), b"pwned");
        let escaped = gate.delete_own(path("/Users/me/.local/share/rosie/../../../.zshrc"));
        let prefixed = gate.write_own_file(path("/Users/me/.config/rosie-evil/x"), b"x");
        let created = gate.create_own_file(path("/Users/me/.zshrc"), b"pwned");

        assert!(matches!(outside, Err(Error::OutsideOwnData { .. })));
        assert!(
            matches!(escaped, Err(Error::OutsideOwnData { path }) if path == Path::new("/Users/me/.zshrc"))
        );
        assert!(matches!(prefixed, Err(Error::OutsideOwnData { .. })));
        assert!(matches!(created, Err(Error::OutsideOwnData { .. })));
        assert!(fake.exists("/Users/me/.zshrc"));
    }

    #[test]
    fn refuses_own_data_through_a_symlink_inside_rosies_folders() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/victim/precious.txt", "x");
        fake.add_dir("/Users/me/.local/share/rosie/packs/real");
        fake.add_symlink(
            "/Users/me/.local/share/rosie/packs/evil",
            "/Users/me/victim",
        );
        let gate = gate(&fake, &[]);
        let evil = Path::new(DATA).join("packs/evil");
        let real = Path::new(DATA).join("packs/real");

        let results = [
            gate.read_own_file(&evil.join("precious.txt")).map(drop),
            gate.write_own_file(&evil.join("planted.txt"), b"x"),
            gate.create_own_file(&evil.join("planted.txt"), b"x"),
            gate.create_own_dir_all(&evil.join("sub")),
            gate.rename_own(&evil, &real.join("moved")),
            gate.rename_own(&real, &evil.join("moved")),
            gate.delete_own(&evil.join("precious.txt")),
            gate.delete_own(&evil),
        ];

        for result in results {
            assert!(
                matches!(&result, Err(Error::Symlink { link, .. }) if *link == evil),
                "expected a symlink refusal, got {result:?}"
            );
        }
        assert!(fake.exists("/Users/me/victim/precious.txt"));
        assert!(!fake.exists("/Users/me/victim/planted.txt"));
        assert!(fake.exists(&evil));
    }

    #[test]
    fn refuses_a_symlinked_own_folder_naming_its_real_path() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/dotfiles/rosie/config.toml", "roots = []");
        fake.add_symlink(CONFIG, "../dotfiles/rosie");
        let gate = gate(&fake, &[]);
        let config = Path::new(CONFIG).join("config.toml");

        let read = gate.read_own_file(&config);
        let written = gate.write_own_file(&config, b"roots = [\"/\"]");

        for result in [read.map(drop), written] {
            assert!(
                matches!(&result, Err(Error::Symlink { real: RealPath::Resolved(real), .. }) if real == Path::new("/Users/me/dotfiles/rosie/config.toml")),
                "expected a symlink refusal, got {result:?}"
            );
        }
    }

    #[test]
    fn refuses_to_delete_an_own_folder_itself() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/.local/share/rosie/packs/builtin/pack.toml", "x");
        let gate = gate(&fake, &[]);

        let result = gate.delete_own(path("/Users/me/.local/share/rosie/."));

        assert!(matches!(result, Err(Error::OwnFolderItself { path }) if path == Path::new(DATA)));
        assert!(fake.exists("/Users/me/.local/share/rosie/packs/builtin/pack.toml"));
    }
}
