//! Contract tier for `Backend::rename`: the fake follows the macOS kernel's rename rules,
//! checked here against `RealBackend` in a temp folder
//! (`docs/spec/testing.md#fakes-and-sandboxing`).

mod common;

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::Path;

use common::{Harness, contract, empty_bounds, error_kind, sorted};
use rosie::fs::{Backend, Error, FileKind, Gate};

contract!(
    renames_a_folder_tree,
    renaming_a_file_onto_itself_keeps_it,
    case_only_rename_of_a_file_respells_it,
    case_only_rename_of_a_non_empty_folder_respells_it,
    renaming_a_folder_into_its_own_subfolder_is_refused,
    renaming_one_hardlink_onto_another_keeps_both,
    renames_in_a_read_only_folder_are_refused,
    a_dot_or_dot_dot_final_name_is_refused,
    moving_a_read_only_folder_to_a_new_parent_is_refused,
    replacing_a_folder_checks_both_folders_not_the_destination_parent,
    a_moved_or_replaced_folder_needs_write_but_not_search_permission,
    a_kind_mismatch_is_reported_before_permission,
    rename_replaces_only_a_destination_of_the_same_kind,
    replacing_an_entry_keeps_the_destination_spelling,
    a_trailing_slash_follows_the_final_link_and_demands_a_folder,
    gate_refuses_to_rename_an_own_folder_itself,
);

fn names(backend: &impl Backend, dir: &Path) -> Vec<OsString> {
    sorted(backend.read_dir(dir).expect("list folder"))
}

fn os(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

fn renames_a_folder_tree(h: &impl Harness) {
    let backend = h.backend();
    let staging = h.root().join("staging");
    let installed = h.root().join("packs/builtin");
    backend
        .create_dir_all(&staging.join("rules"))
        .expect("create staging");
    backend
        .write_file(&staging.join("rules/node.toml"), b"x")
        .expect("write");
    backend
        .create_dir_all(&h.root().join("packs"))
        .expect("create packs");

    backend.rename(&staging, &installed).expect("rename");

    assert_eq!(error_kind(backend.lstat(&staging)), ErrorKind::NotFound);
    assert_eq!(
        backend
            .read_file(&installed.join("rules/node.toml"))
            .expect("moved"),
        b"x"
    );
}

fn renaming_a_file_onto_itself_keeps_it(h: &impl Harness) {
    let backend = h.backend();
    let file = h.root().join("keep.txt");
    backend.write_file(&file, b"same").expect("write");

    backend.rename(&file, &file).expect("self-rename");

    assert_eq!(backend.read_file(&file).expect("still there"), b"same");
}

fn case_only_rename_of_a_file_respells_it(h: &impl Harness) {
    let backend = h.backend();
    let original = h.root().join("x.txt");
    let respelled = h.root().join("X.txt");
    backend.write_file(&original, b"data").expect("write");

    backend
        .rename(&original, &respelled)
        .expect("case-only rename");

    let names = sorted(
        backend
            .read_dir(h.root())
            .expect("read root")
            .into_iter()
            .collect(),
    );
    assert_eq!(names, vec![OsString::from("X.txt")]);
    assert_eq!(backend.read_file(&respelled).expect("read"), b"data");
}

fn case_only_rename_of_a_non_empty_folder_respells_it(h: &impl Harness) {
    let backend = h.backend();
    let original = h.root().join("dir");
    let respelled = h.root().join("Dir");
    backend
        .create_dir_all(&original.join("child"))
        .expect("create nested folder");
    backend
        .write_file(&original.join("child/file.txt"), b"nested")
        .expect("write");

    backend
        .rename(&original, &respelled)
        .expect("case-only rename");

    let names = sorted(
        backend
            .read_dir(h.root())
            .expect("read root")
            .into_iter()
            .collect(),
    );
    assert_eq!(names, vec![OsString::from("Dir")]);
    assert_eq!(
        backend
            .read_file(&respelled.join("child/file.txt"))
            .expect("nested file still readable"),
        b"nested"
    );
}

fn renaming_a_folder_into_its_own_subfolder_is_refused(h: &impl Harness) {
    let backend = h.backend();
    let parent = h.root().join("parent");
    let child = parent.join("child");
    backend
        .create_dir_all(&child)
        .expect("create nested folder");
    backend
        .write_file(&child.join("marker.txt"), b"still here")
        .expect("write");
    let moved = child.join("moved");

    let refused = backend.rename(&parent, &moved);

    assert_eq!(error_kind(refused), ErrorKind::InvalidInput);
    assert_eq!(
        backend
            .read_file(&child.join("marker.txt"))
            .expect("tree intact"),
        b"still here"
    );
}

fn renaming_one_hardlink_onto_another_keeps_both(h: &impl Harness) {
    let backend = h.backend();
    let one = h.root().join("one");
    let two = h.root().join("two");
    backend.write_file(&one, b"shared").expect("write");
    h.hardlink(&one, &two);

    backend.rename(&one, &two).expect("same-file rename");

    assert_eq!(names(backend, h.root()), os(&["one", "two"]));
    assert_eq!(backend.read_file(&one).expect("read one"), b"shared");
}

// A rename needs write permission on the folder it removes the old name from, even
// when nothing would change.

fn renames_in_a_read_only_folder_are_refused(h: &impl Harness) {
    let backend = h.backend();
    let locked = h.root().join("locked");
    let file = locked.join("x.txt");
    backend.create_dir_all(&locked).expect("create");
    backend.write_file(&file, b"x").expect("write");
    backend.set_mode(&locked, 0o555).expect("make read-only");

    let self_rename = backend.rename(&file, &file);
    let case_only = backend.rename(&file, &locked.join("X.txt"));

    assert_eq!(error_kind(self_rename), ErrorKind::PermissionDenied);
    assert_eq!(error_kind(case_only), ErrorKind::PermissionDenied);
    assert_eq!(names(backend, &locked), os(&["x.txt"]));
}

fn a_dot_or_dot_dot_final_name_is_refused(h: &impl Harness) {
    let backend = h.backend();
    let parent = h.root().join("parent");
    let moving = parent.join("moving");
    let sub = moving.join("sub");
    let other = parent.join("other");
    backend.create_dir_all(&sub).expect("create");
    backend.create_dir_all(&other).expect("create");
    // Read-only, so the refusal is shown to come before the permission check.
    backend.set_mode(&parent, 0o555).expect("make read-only");

    let results = [
        backend.rename(&moving, &sub.join("..")),
        backend.rename(&moving, &other.join(".")),
        backend.rename(&moving.join("."), &parent.join("new")),
        backend.rename(&sub.join(".."), &parent.join("new")),
    ];
    // The lookup comes first: `.` is looked up inside a folder that must exist.
    let dot_in_missing = backend.rename(&moving, &parent.join("missing/."));

    for result in results {
        assert_eq!(error_kind(result), ErrorKind::InvalidInput);
    }
    assert_eq!(error_kind(dot_in_missing), ErrorKind::NotFound);
    assert_eq!(names(backend, &parent), os(&["moving", "other"]));
    assert_eq!(names(backend, &moving), os(&["sub"]));
}

// A folder moved to a new parent, or renamed over an existing folder, has its `..`
// entry rewritten, so it needs write permission on itself.

fn moving_a_read_only_folder_to_a_new_parent_is_refused(h: &impl Harness) {
    let backend = h.backend();
    let folder = h.root().join("a/pack");
    let elsewhere = h.root().join("b");
    backend.create_dir_all(&folder).expect("create");
    backend.create_dir_all(&elsewhere).expect("create");
    backend
        .write_file(&folder.join("pack.toml"), b"x")
        .expect("write");
    backend.set_mode(&folder, 0o555).expect("make read-only");

    let moved = backend.rename(&folder, &elsewhere.join("pack"));
    let renamed_in_place = backend.rename(&folder, &h.root().join("a/renamed"));

    assert_eq!(error_kind(moved), ErrorKind::PermissionDenied);
    renamed_in_place.expect("same parent needs no write permission on the folder");
    assert_eq!(names(backend, &h.root().join("a")), os(&["renamed"]));
    assert_eq!(names(backend, &elsewhere), os(&[]));
}

fn replacing_a_folder_checks_both_folders_not_the_destination_parent(h: &impl Harness) {
    let backend = h.backend();
    let root = h.root();
    for dir in [
        "source",
        "locked-target",
        "locked-source",
        "target",
        "x/d",
        "y/d",
        "full",
    ] {
        backend.create_dir_all(&root.join(dir)).expect("create");
    }
    backend
        .write_file(&root.join("full/keep"), b"x")
        .expect("write");
    for dir in ["locked-target", "locked-source", "y", "full"] {
        backend
            .set_mode(&root.join(dir), 0o555)
            .expect("make read-only");
    }

    let onto_locked = backend.rename(&root.join("source"), &root.join("locked-target"));
    let from_locked = backend.rename(&root.join("locked-source"), &root.join("target"));
    let onto_full_locked = backend.rename(&root.join("source"), &root.join("full"));
    let into_locked_parent = backend.rename(&root.join("x/d"), &root.join("y/d"));

    assert_eq!(error_kind(onto_locked), ErrorKind::PermissionDenied);
    assert_eq!(error_kind(from_locked), ErrorKind::PermissionDenied);
    assert_eq!(error_kind(onto_full_locked), ErrorKind::PermissionDenied);
    into_locked_parent.expect("the destination's parent is not checked");
    assert_eq!(names(backend, &root.join("x")), os(&[]));
    assert_eq!(names(backend, &root.join("y")), os(&["d"]));
}

fn a_moved_or_replaced_folder_needs_write_but_not_search_permission(h: &impl Harness) {
    let backend = h.backend();
    let root = h.root();
    for dir in ["a/moving", "b", "source", "write-only"] {
        backend.create_dir_all(&root.join(dir)).expect("create");
    }
    for dir in ["a/moving", "write-only"] {
        backend
            .set_mode(&root.join(dir), 0o255)
            .expect("make write-only");
    }

    backend
        .rename(&root.join("a/moving"), &root.join("b/moving"))
        .expect("move a write-only folder");
    backend
        .rename(&root.join("source"), &root.join("write-only"))
        .expect("replace a write-only folder");

    assert_eq!(names(backend, &root.join("b")), os(&["moving"]));
    assert_eq!(names(backend, root), os(&["a", "b", "write-only"]));
}

fn a_kind_mismatch_is_reported_before_permission(h: &impl Harness) {
    let backend = h.backend();
    let locked = h.root().join("locked");
    backend.create_dir_all(&locked.join("dir")).expect("create");
    backend
        .create_dir_all(&locked.join("empty"))
        .expect("create");
    backend
        .write_file(&locked.join("file"), b"x")
        .expect("write");
    backend.set_mode(&locked, 0o555).expect("make read-only");

    let dir_onto_file = backend.rename(&locked.join("dir"), &locked.join("file"));
    let file_onto_dir = backend.rename(&locked.join("file"), &locked.join("empty"));

    assert_eq!(error_kind(dir_onto_file), ErrorKind::NotADirectory);
    assert_eq!(error_kind(file_onto_dir), ErrorKind::IsADirectory);
}

fn rename_replaces_only_a_destination_of_the_same_kind(h: &impl Harness) {
    let backend = h.backend();
    let root = h.root();
    for dir in ["dir", "empty", "full/child", "target"] {
        backend.create_dir_all(&root.join(dir)).expect("create");
    }
    for file in ["file", "other-file", "new-file", "target/keep"] {
        backend
            .write_file(&root.join(file), file.as_bytes())
            .expect("write");
    }
    h.symlink(&root.join("link"), &root.join("target"));

    let file_onto_dir = backend.rename(&root.join("file"), &root.join("empty"));
    let dir_onto_file = backend.rename(&root.join("dir"), &root.join("file"));
    let dir_onto_full = backend.rename(&root.join("dir"), &root.join("full"));
    let dir_onto_link = backend.rename(&root.join("dir"), &root.join("link"));
    backend
        .rename(&root.join("file"), &root.join("other-file"))
        .expect("file replaces file");
    backend
        .rename(&root.join("new-file"), &root.join("link"))
        .expect("file replaces the link itself");
    backend
        .rename(&root.join("dir"), &root.join("empty"))
        .expect("folder replaces empty folder");

    assert_eq!(error_kind(file_onto_dir), ErrorKind::IsADirectory);
    assert_eq!(error_kind(dir_onto_file), ErrorKind::NotADirectory);
    assert_eq!(error_kind(dir_onto_full), ErrorKind::DirectoryNotEmpty);
    assert_eq!(error_kind(dir_onto_link), ErrorKind::NotADirectory);
    assert_eq!(
        names(backend, root),
        os(&["empty", "full", "link", "other-file", "target"])
    );
    assert_eq!(
        backend.read_file(&root.join("other-file")).expect("read"),
        b"file"
    );
    assert_eq!(
        backend.lstat(&root.join("link")).expect("lstat").kind,
        FileKind::File
    );
    assert_eq!(names(backend, &root.join("target")), os(&["keep"]));
}

fn replacing_an_entry_keeps_the_destination_spelling(h: &impl Harness) {
    let backend = h.backend();
    let a = h.root().join("a");
    let b = h.root().join("b");
    backend.create_dir_all(&a).expect("create");
    backend.create_dir_all(&b).expect("create");
    backend.write_file(&a.join("x"), b"new").expect("write");
    backend.write_file(&b.join("x"), b"old").expect("write");

    backend
        .rename(&a.join("x"), &b.join("X"))
        .expect("replace across folders");

    assert_eq!(names(backend, &b), os(&["x"]));
    assert_eq!(backend.read_file(&b.join("x")).expect("read"), b"new");
}

fn a_trailing_slash_follows_the_final_link_and_demands_a_folder(h: &impl Harness) {
    let backend = h.backend();
    let root = h.root();
    backend.write_file(&root.join("file"), b"x").expect("write");
    backend
        .create_dir_all(&root.join("target"))
        .expect("create");
    h.symlink(&root.join("link"), Path::new("target"));

    let file_as_folder = backend.rename(&root.join("file/"), &root.join("moved"));
    let file_to_folder_name = backend.rename(&root.join("file"), &root.join("moved/"));
    backend
        .rename(&root.join("link/"), &root.join("renamed"))
        .expect("rename the link's target");

    assert_eq!(error_kind(file_as_folder), ErrorKind::NotADirectory);
    assert_eq!(error_kind(file_to_folder_name), ErrorKind::NotFound);
    assert_eq!(names(backend, root), os(&["file", "link", "renamed"]));
    assert_eq!(
        backend.lstat(&root.join("link")).expect("lstat").kind,
        FileKind::Symlink
    );
}

fn gate_refuses_to_rename_an_own_folder_itself(h: &impl Harness) {
    let backend = h.backend();
    let bounds = empty_bounds(h);
    let (config, data) = (bounds.config_dir.clone(), bounds.data_dir.clone());
    let staged = data.join("staging");
    backend.create_dir_all(&config).expect("create config");
    backend.create_dir_all(&staged).expect("create staging");
    backend
        .write_file(&data.join("packs.toml"), b"x")
        .expect("write");
    let gate = Gate::new(backend, bounds).expect("gate");

    let moved_away = gate.rename_own(&data, &config.join("moved"));
    let replaced = gate.rename_own(&staged, &config);

    for result in [moved_away, replaced] {
        assert!(
            matches!(result, Err(Error::OwnFolderItself { .. })),
            "{result:?}"
        );
    }
    assert_eq!(names(backend, &config), os(&[]));
    assert_eq!(names(backend, &data), os(&["packs.toml", "staging"]));
}
