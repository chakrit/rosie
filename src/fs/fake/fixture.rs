//! Fixture setup: building the volume a test starts from.
//!
//! Fixture methods bypass permissions and are not recorded. They keep every guarantee a
//! case-insensitive APFS volume gives and panic on a fixture that would break one,
//! because such a fixture is a bug in the test setup:
//!
//! - A fixture path is absolute and plain: no `.` or `..`, and no symlink or non-folder
//!   before its last name. Missing folders on the way are created.
//! - Every name is valid UTF-8 and at most 255 bytes long.
//! - Names in one folder are unique after case folding, and a fixture names an existing
//!   entry by the spelling it was created with.
//! - A hardlink names a non-folder on the volume of the folder it goes in.
//! - A mount point is an existing folder other than `/`. Its subtree holds one volume,
//!   none of its inodes is also named outside it, and no other volume uses its device.
//!
//! Every path lookup goes through [`State::fixture_child`], which folds case the same
//! way the kernel path walk does.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use super::{
    Body, DIR_MODE, FILE_MODE, FakeBackend, Node, ROOT_UID, SF_DATALESS, SYMLINK_MODE, State,
    USER_UID, allocation_for,
};

/// The longest name APFS stores, in UTF-8 bytes.
const NAME_MAX: usize = 255;

impl FakeBackend {
    /// Adds a folder and any missing parents, owned by the user.
    pub fn add_dir(&self, path: impl AsRef<Path>) {
        self.lock().fixture_dir_all(path.as_ref());
    }

    /// Adds a regular file with contents, creating missing parents.
    pub fn add_file(&self, path: impl AsRef<Path>, contents: impl Into<Vec<u8>>) {
        let contents = contents.into();
        let allocated = allocation_for(contents.len());
        self.add_node(path.as_ref(), Body::File(contents), allocated, FILE_MODE);
    }

    /// Adds a regular file that reports `allocated` bytes on disk without holding them.
    pub fn add_sized_file(&self, path: impl AsRef<Path>, allocated: u64) {
        self.add_node(path.as_ref(), Body::File(Vec::new()), allocated, FILE_MODE);
    }

    pub fn add_symlink(&self, path: impl AsRef<Path>, target: impl Into<PathBuf>) {
        self.add_node(path.as_ref(), Body::Symlink(target.into()), 0, SYMLINK_MODE);
    }

    /// Adds a second name for an existing file; both share one inode.
    pub fn add_hardlink(&self, existing: impl AsRef<Path>, new: impl AsRef<Path>) {
        let mut state = self.lock();
        let (existing, new) = (existing.as_ref(), new.as_ref());
        let source = state.fixture_entry(existing);
        assert!(
            !state.is_dir(&source),
            "fixture hardlink source {} is a folder; APFS forbids user folder hardlinks",
            existing.display()
        );

        let key = state.fixture_vacancy(new);
        let source_dev = state.node_at(&source).dev;
        let folder_dev = state.node_at(folder_of(&key)).dev;
        assert!(
            source_dev == folder_dev,
            "fixture hardlink {} to {} links across volumes (dev {folder_dev} and {source_dev})",
            new.display(),
            existing.display()
        );

        let inode = state.entries[&source];
        state.entries.insert(key, inode);
    }

    pub fn chown(&self, path: impl AsRef<Path>, uid: u32) {
        self.lock().fixture_node(path.as_ref()).uid = uid;
    }

    /// Sets the owner of `path` and of every folder above it but `/`, such as a
    /// root-owned system folder chain.
    pub fn chown_with_ancestors(&self, path: impl AsRef<Path>, uid: u32) {
        let path = path.as_ref();
        let mut state = self.lock();
        for entry in path.ancestors().filter(|entry| *entry != Path::new("/")) {
            state.fixture_node(entry).uid = uid;
        }
    }

    pub fn chmod(&self, path: impl AsRef<Path>, mode: u32) {
        self.lock().fixture_node(path.as_ref()).mode = mode & 0o7777;
    }

    /// Puts a folder and everything under it on another volume. Entries added below it
    /// later inherit the volume.
    pub fn mount(&self, path: impl AsRef<Path>, dev: u64) {
        let mut state = self.lock();
        let point = state.fixture_entry(path.as_ref());
        state.require_mountable(&point, dev);

        let inodes: Vec<u64> = state.subtree(&point).map(|(_, inode)| inode).collect();
        for inode in inodes {
            state
                .inodes
                .get_mut(&inode)
                .expect("fixture inode exists")
                .dev = dev;
        }
    }

    /// Marks a file or folder as a dataless cloud placeholder.
    pub fn make_dataless(&self, path: impl AsRef<Path>) {
        let mut state = self.lock();
        let path = path.as_ref();
        let node = state.fixture_node(path);
        assert!(
            !matches!(node.body, Body::Symlink(_)),
            "fixture {} is a symlink; only files and folders can be dataless",
            path.display()
        );
        node.flags |= SF_DATALESS;
    }

    fn add_node(&self, path: &Path, body: Body, allocated: u64, mode: u32) {
        let mut state = self.lock();
        let key = state.fixture_vacancy(path);

        let dev = state.node_at(folder_of(&key)).dev;
        let uid = state.user_uid_or_default();
        let mut node = Node::new(body, dev, uid, mode);
        node.allocated = allocated;
        state.link_new(key, node);
    }
}

impl State {
    // lookups

    /// The stored key of an existing entry.
    fn fixture_entry(&self, path: &Path) -> PathBuf {
        let mut current = PathBuf::from("/");
        for name in fixture_names(path) {
            current = self
                .fixture_step(path, &current, name)
                .unwrap_or_else(|| panic!("fixture {} does not exist", path.display()));
        }
        current
    }

    /// The key a new entry at `path` takes, after creating its missing parent folders.
    /// Nothing may hold the name yet, under any spelling.
    fn fixture_vacancy(&mut self, path: &Path) -> PathBuf {
        let names = fixture_names(path);
        let Some((last, parents)) = names.split_last() else {
            panic!("fixture {} is the root", path.display());
        };

        let folder = self.fixture_dirs(path, parents);
        if let Some(existing) = self.fixture_child(path, &folder, last) {
            panic!(
                "fixture {} already exists as {}",
                path.display(),
                existing.display()
            );
        }
        folder.join(last)
    }

    /// Walks one name of a fixture path, which must use the stored spelling.
    fn fixture_step(&self, path: &Path, folder: &Path, name: &OsStr) -> Option<PathBuf> {
        let stored = self.fixture_child(path, folder, name)?;
        let typed = folder.join(name);
        assert!(
            stored == typed,
            "fixture {} spells {} as {}",
            path.display(),
            stored.display(),
            typed.display()
        );
        Some(stored)
    }

    /// The entry `name` in `folder`, under any spelling, found as the kernel path walk
    /// finds it. `folder` must be a folder.
    fn fixture_child(&self, path: &Path, folder: &Path, name: &OsStr) -> Option<PathBuf> {
        assert!(
            self.is_dir(folder),
            "fixture {}: {} is not a folder",
            path.display(),
            folder.display()
        );
        self.child_named(folder, name)
    }

    fn fixture_node(&mut self, path: &Path) -> &mut Node {
        let key = self.fixture_entry(path);
        self.node_mut(&key)
    }

    // creation

    /// Adds a folder and any missing parents; returns its stored key.
    fn fixture_dir_all(&mut self, path: &Path) -> PathBuf {
        let names = fixture_names(path);
        let key = self.fixture_dirs(path, &names);
        assert!(
            self.is_dir(&key),
            "fixture {} is not a folder",
            path.display()
        );
        key
    }

    /// Walks `names` from `/`, creating each missing folder, and returns the key reached.
    fn fixture_dirs(&mut self, path: &Path, names: &[&OsStr]) -> PathBuf {
        let mut current = PathBuf::from("/");
        for name in names {
            current = match self.fixture_step(path, &current, name) {
                Some(child) => child,
                None => self.fixture_new_dir(current.join(name)),
            };
        }
        current
    }

    fn fixture_new_dir(&mut self, key: PathBuf) -> PathBuf {
        let dev = self.node_at(folder_of(&key)).dev;
        let node = Node::new(Body::Dir, dev, self.user_uid_or_default(), DIR_MODE);
        self.link_new(key.clone(), node);
        key
    }

    fn user_uid_or_default(&self) -> u32 {
        match self.user_uid {
            ROOT_UID => USER_UID,
            user => user,
        }
    }

    // mounts

    fn require_mountable(&self, point: &Path, dev: u64) {
        assert!(
            point != Path::new("/"),
            "fixture mount point / is the root volume"
        );
        assert!(
            self.is_dir(point),
            "fixture mount point {} is not a folder",
            point.display()
        );

        let point_dev = self.node_at(point).dev;
        let inside: HashSet<u64> = self.subtree(point).map(|(_, inode)| inode).collect();
        let single_volume = inside
            .iter()
            .all(|inode| self.inodes[inode].dev == point_dev);
        assert!(
            single_volume,
            "fixture mount point {} holds another volume",
            point.display()
        );

        let outside = self
            .entries
            .iter()
            .filter(|(key, _)| !key.starts_with(point));
        for (key, inode) in outside {
            assert!(
                !inside.contains(inode),
                "fixture mount point {} holds an inode also named outside it, at {}",
                point.display(),
                key.display()
            );
            assert!(
                self.inodes[inode].dev != dev,
                "fixture device {dev} is already in use at {}",
                key.display()
            );
        }
    }
}

/// The names of a fixture path, each checked against what APFS stores.
fn fixture_names(path: &Path) -> Vec<&OsStr> {
    assert!(
        path.has_root(),
        "fixture path {} must be absolute and plain",
        path.display()
    );
    path.components()
        .filter(|component| *component != Component::RootDir)
        .map(|component| match component {
            Component::Normal(name) => fixture_name(path, name),
            _ => panic!("fixture path {} must be absolute and plain", path.display()),
        })
        .collect()
}

fn fixture_name<'a>(path: &Path, name: &'a OsStr) -> &'a OsStr {
    let Some(utf8) = name.to_str() else {
        panic!(
            "fixture path {} has a name that is not valid UTF-8",
            path.display()
        );
    };
    assert!(
        utf8.len() <= NAME_MAX,
        "fixture path {} has a name longer than {NAME_MAX} bytes",
        path.display()
    );
    name
}

/// The folder a stored key sits in; fixture keys below `/` always have one.
fn folder_of(key: &Path) -> &Path {
    key.parent()
        .expect("fixture key below the root has a folder")
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use crate::fs::backend::Backend;
    use crate::fs::fake::FakeBackend;

    // names and spellings

    #[test]
    #[should_panic(expected = "already exists as /a/One")]
    fn a_case_variant_of_an_existing_name_panics() {
        let fake = FakeBackend::new();
        fake.add_file("/a/One", "x");

        fake.add_file("/a/one", "y");
    }

    #[test]
    #[should_panic(expected = "spells /Users/me/Code as /Users/me/code")]
    fn a_folder_respelled_in_a_later_fixture_panics() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/Code/x", "x");

        fake.add_file("/Users/me/code/y", "y");
    }

    #[test]
    #[should_panic(expected = "spells /a/one as /a/ONE")]
    fn an_existing_entry_named_by_another_spelling_panics() {
        let fake = FakeBackend::new();
        fake.add_file("/a/one", "x");

        fake.chown("/a/ONE", 0);
    }

    #[test]
    #[should_panic(expected = "not valid UTF-8")]
    fn a_name_that_is_not_utf8_panics() {
        let fake = FakeBackend::new();

        fake.add_file(Path::new(OsStr::from_bytes(b"/a/\xff")), "x");
    }

    #[test]
    #[should_panic(expected = "longer than 255 bytes")]
    fn a_name_longer_than_the_volume_allows_panics() {
        let fake = FakeBackend::new();
        let long = format!("/a/{}", "n".repeat(256));

        fake.add_file(long, "x");
    }

    #[test]
    #[should_panic(expected = "must be absolute and plain")]
    fn a_relative_fixture_path_panics() {
        let fake = FakeBackend::new();

        fake.add_file("a/b", "x");
    }

    #[test]
    #[should_panic(expected = "must be absolute and plain")]
    fn a_fixture_path_with_dot_dot_panics() {
        let fake = FakeBackend::new();
        fake.add_dir("/a/b");

        fake.add_file("/a/b/../c", "x");
    }

    #[test]
    #[should_panic(expected = "is the root")]
    fn adding_an_entry_at_the_root_panics() {
        let fake = FakeBackend::new();

        fake.add_file("/", "x");
    }

    // parents

    #[test]
    #[should_panic(expected = "is not a folder")]
    fn an_entry_below_a_file_panics() {
        let fake = FakeBackend::new();
        fake.add_file("/a/f", "x");

        fake.add_file("/a/f/x", "y");
    }

    #[test]
    #[should_panic(expected = "is not a folder")]
    fn an_entry_below_a_symlink_panics() {
        let fake = FakeBackend::new();
        fake.add_dir("/b");
        fake.add_symlink("/a/link", "/b");

        fake.add_file("/a/link/x", "y");
    }

    // hardlinks

    #[test]
    #[should_panic(expected = "already exists")]
    fn hardlink_onto_an_existing_path_panics() {
        let fake = FakeBackend::new();
        fake.add_file("/a/one", "data");
        fake.add_file("/b/two", "other");

        fake.add_hardlink("/a/one", "/b/two");
    }

    #[test]
    #[should_panic(expected = "already exists as /b/two")]
    fn hardlink_onto_a_case_variant_panics() {
        let fake = FakeBackend::new();
        fake.add_file("/a/one", "data");
        fake.add_file("/b/two", "other");

        fake.add_hardlink("/a/one", "/b/TWO");
    }

    #[test]
    #[should_panic(expected = "forbids user folder hardlinks")]
    fn hardlink_of_a_folder_panics() {
        let fake = FakeBackend::new();
        fake.add_dir("/a/dir");
        fake.add_file("/a/dir/child", "data");

        fake.add_hardlink("/a/dir", "/b/dir2");
    }

    #[test]
    #[should_panic(expected = "across volumes")]
    fn hardlink_into_another_volume_panics() {
        let fake = FakeBackend::new();
        fake.add_file("/a/one", "data");
        fake.add_dir("/Volumes/ext");
        fake.mount("/Volumes/ext", 7);

        fake.add_hardlink("/a/one", "/Volumes/ext/two");
    }

    // mounts

    #[test]
    #[should_panic(expected = "does not exist")]
    fn mount_of_a_missing_path_panics() {
        let fake = FakeBackend::new();

        fake.mount("/missing", 7);
    }

    #[test]
    #[should_panic(expected = "is not a folder")]
    fn mount_of_a_file_panics() {
        let fake = FakeBackend::new();
        fake.add_file("/m", "x");

        fake.mount("/m", 7);
    }

    #[test]
    #[should_panic(expected = "the root volume")]
    fn mount_of_the_root_panics() {
        let fake = FakeBackend::new();

        fake.mount("/", 7);
    }

    #[test]
    #[should_panic(expected = "also named outside")]
    fn mount_over_an_inode_named_outside_it_panics() {
        let fake = FakeBackend::new();
        fake.add_file("/a/one", "data");
        fake.add_hardlink("/a/one", "/m/two");

        fake.mount("/m", 7);
    }

    #[test]
    #[should_panic(expected = "already in use")]
    fn mount_with_a_device_used_elsewhere_panics() {
        let fake = FakeBackend::new();
        fake.add_dir("/Volumes/a");
        fake.add_dir("/Volumes/b");
        fake.mount("/Volumes/a", 7);

        fake.mount("/Volumes/b", 7);
    }

    #[test]
    #[should_panic(expected = "already in use")]
    fn mount_with_the_root_device_panics() {
        let fake = FakeBackend::new();
        fake.add_dir("/Volumes/a");
        let root_dev = fake.lstat(Path::new("/")).expect("lstat root").dev;

        fake.mount("/Volumes/a", root_dev);
    }

    #[test]
    #[should_panic(expected = "holds another volume")]
    fn mount_over_a_nested_volume_panics() {
        let fake = FakeBackend::new();
        fake.add_dir("/v/inner");
        fake.mount("/v/inner", 8);

        fake.mount("/v", 7);
    }

    // other attributes

    #[test]
    #[should_panic(expected = "is a symlink")]
    fn dataless_symlink_panics() {
        let fake = FakeBackend::new();
        fake.add_symlink("/a/link", "/b");

        fake.make_dataless("/a/link");
    }
}
