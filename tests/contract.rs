//! Contract tier: the in-memory fake and `RealBackend` behave the same for file
//! operations (`docs/spec/testing.md#fakes-and-sandboxing`). Rename has its own file,
//! `contract_rename.rs`.

mod common;

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use common::{Harness, RealHarness, contract, empty_bounds, error_kind, sorted};
use rosie::fs::{Backend, Bounds, Error, FileKind, Gate, RealBackend};

contract!(
    writes_reads_and_overwrites_files,
    lists_folder_entries_by_name,
    lstat_reports_the_entry_itself,
    hardlinks_share_an_inode,
    read_link_returns_the_stored_target,
    removing_a_symlink_leaves_its_target,
    removes_only_empty_folders,
    read_only_folders_refuse_removal_until_made_writable,
    missing_paths_are_not_found,
    listing_a_file_is_not_a_directory,
    gate_deletes_a_tree_with_links_and_read_only_folders,
    paths_through_a_linked_folder_reach_its_target,
    writes_land_inside_a_linked_folder,
    writing_through_a_dangling_link_creates_its_target,
    nothing_can_sit_below_a_file,
    removing_a_folder_as_a_file_is_refused,
    a_symlink_loop_fails_to_resolve,
    gate_refuses_own_data_through_a_symlink,
    a_trailing_slash_or_dot_follows_the_final_link_and_demands_a_folder,
);

fn writes_reads_and_overwrites_files(h: &impl Harness) {
    let file = h.root().join("config.toml");

    h.backend()
        .write_file(&file, b"first version")
        .expect("write");
    h.backend().write_file(&file, b"second").expect("overwrite");

    assert_eq!(h.backend().read_file(&file).expect("read"), b"second");
}

fn lists_folder_entries_by_name(h: &impl Harness) {
    let backend = h.backend();
    backend
        .create_dir_all(&h.root().join("a/nested/deep"))
        .expect("create nested");
    backend
        .write_file(&h.root().join("a/file.txt"), b"x")
        .expect("write");
    h.symlink(&h.root().join("a/link"), Path::new("nested"));

    let names = sorted(backend.read_dir(&h.root().join("a")).expect("list"));

    let expected: Vec<OsString> = ["file.txt", "link", "nested"].map(OsString::from).into();
    assert_eq!(names, expected);
}

fn lstat_reports_the_entry_itself(h: &impl Harness) {
    let backend = h.backend();
    let dir = h.root().join("dir");
    let file = h.root().join("file.bin");
    let link = h.root().join("link");
    backend.create_dir_all(&dir).expect("create dir");
    backend.write_file(&file, &[7; 10_000]).expect("write");
    h.symlink(&link, &dir);

    let dir_meta = backend.lstat(&dir).expect("lstat dir");
    let file_meta = backend.lstat(&file).expect("lstat file");
    let link_meta = backend.lstat(&link).expect("lstat link");

    assert_eq!(
        (dir_meta.kind, file_meta.kind, link_meta.kind),
        (FileKind::Dir, FileKind::File, FileKind::Symlink)
    );
    assert!(file_meta.allocated >= 10_000);
    assert_eq!(file_meta.dev, dir_meta.dev);
    assert_ne!(file_meta.inode, dir_meta.inode);
    assert_eq!(file_meta.uid, h.user_uid());
    assert!(!file_meta.is_dataless());
}

fn hardlinks_share_an_inode(h: &impl Harness) {
    let first = h.root().join("first");
    let second = h.root().join("second");
    h.backend().write_file(&first, b"shared").expect("write");
    h.hardlink(&first, &second);

    let first_meta = h.backend().lstat(&first).expect("lstat first");
    let second_meta = h.backend().lstat(&second).expect("lstat second");

    assert_eq!(
        (first_meta.dev, first_meta.inode),
        (second_meta.dev, second_meta.inode)
    );
}

fn read_link_returns_the_stored_target(h: &impl Harness) {
    let link = h.root().join("link");
    h.symlink(&link, Path::new("../elsewhere/target"));

    let target = h.backend().read_link(&link).expect("read link");

    assert_eq!(target, PathBuf::from("../elsewhere/target"));
}

fn removing_a_symlink_leaves_its_target(h: &impl Harness) {
    let backend = h.backend();
    let target_dir = h.root().join("source");
    let target_file = target_dir.join("keep.txt");
    let dir_link = h.root().join("dir-link");
    let file_link = h.root().join("file-link");
    backend.create_dir_all(&target_dir).expect("create dir");
    backend.write_file(&target_file, b"keep").expect("write");
    h.symlink(&dir_link, &target_dir);
    h.symlink(&file_link, &target_file);

    backend.remove_file(&dir_link).expect("remove dir link");
    backend.remove_file(&file_link).expect("remove file link");

    assert_eq!(error_kind(backend.lstat(&dir_link)), ErrorKind::NotFound);
    assert_eq!(error_kind(backend.lstat(&file_link)), ErrorKind::NotFound);
    assert_eq!(
        backend.read_file(&target_file).expect("target intact"),
        b"keep"
    );
}

fn removes_only_empty_folders(h: &impl Harness) {
    let backend = h.backend();
    let dir = h.root().join("dir");
    let file = dir.join("file");
    backend.create_dir_all(&dir).expect("create");
    backend.write_file(&file, b"x").expect("write");

    let refused = backend.remove_empty_dir(&dir);
    backend.remove_file(&file).expect("remove file");
    backend.remove_empty_dir(&dir).expect("remove empty folder");

    assert_eq!(error_kind(refused), ErrorKind::DirectoryNotEmpty);
    assert_eq!(error_kind(backend.lstat(&dir)), ErrorKind::NotFound);
}

fn read_only_folders_refuse_removal_until_made_writable(h: &impl Harness) {
    let backend = h.backend();
    let dir = h.root().join("module@v1");
    let file = dir.join("go.mod");
    backend.create_dir_all(&dir).expect("create");
    backend.write_file(&file, b"module x").expect("write");
    backend.set_mode(&dir, 0o555).expect("make read-only");

    let refused = backend.remove_file(&file);
    let mode = backend.lstat(&dir).expect("lstat").mode;
    backend.set_mode(&dir, 0o755).expect("make writable");
    backend.remove_file(&file).expect("remove after chmod");

    assert_eq!(error_kind(refused), ErrorKind::PermissionDenied);
    assert_eq!(mode, 0o555);
}

fn missing_paths_are_not_found(h: &impl Harness) {
    let missing = h.root().join("missing");

    assert_eq!(error_kind(h.backend().lstat(&missing)), ErrorKind::NotFound);
    assert_eq!(
        error_kind(h.backend().read_dir(&missing)),
        ErrorKind::NotFound
    );
    assert_eq!(
        error_kind(h.backend().read_file(&missing)),
        ErrorKind::NotFound
    );
}

fn listing_a_file_is_not_a_directory(h: &impl Harness) {
    let file = h.root().join("file");
    h.backend().write_file(&file, b"x").expect("write");

    assert_eq!(
        error_kind(h.backend().read_dir(&file)),
        ErrorKind::NotADirectory
    );
}

fn gate_deletes_a_tree_with_links_and_read_only_folders(h: &impl Harness) {
    let backend = h.backend();
    let outside = h.root().join("outside/keep.txt");
    let code = h.root().join("code");
    let target = code.join("app/node_modules");
    backend
        .create_dir_all(&target.join("pkg/lib"))
        .expect("create tree");
    backend
        .create_dir_all(&h.root().join("outside"))
        .expect("create outside");
    backend
        .write_file(&outside, b"keep")
        .expect("write outside");
    backend
        .write_file(&target.join("pkg/lib/index.js"), b"x")
        .expect("write inside");
    h.symlink(&target.join("linked"), &h.root().join("outside"));
    backend
        .set_mode(&target.join("pkg/lib"), 0o555)
        .expect("make read-only");
    let bounds = Bounds {
        roots: vec![code],
        config_dir: h.root().join("config/rosie"),
        data_dir: h.root().join("data/rosie"),
        user_uid: h.user_uid(),
    };
    let gate = Gate::new(backend, bounds).expect("gate");

    gate.delete(&target).expect("delete tree");
    let refused = gate.delete(&outside);

    assert_eq!(error_kind(backend.lstat(&target)), ErrorKind::NotFound);
    assert_eq!(
        backend.read_file(&outside).expect("link target intact"),
        b"keep"
    );
    assert!(matches!(refused, Err(Error::OutsideRoots { .. })));
}

// Paths through symlinks. The gate refuses them for mutation, but reads and the
// backend itself pass them to the kernel, which follows every non-final link.

fn paths_through_a_linked_folder_reach_its_target(h: &impl Harness) {
    let backend = h.backend();
    let source = h.root().join("source");
    let link = h.root().join("link");
    backend.create_dir_all(&source).expect("create");
    backend
        .write_file(&source.join("file.txt"), b"through")
        .expect("write");
    h.symlink(&link, Path::new("source"));

    let meta = backend
        .lstat(&link.join("file.txt"))
        .expect("lstat through link");
    let contents = backend
        .read_file(&link.join("file.txt"))
        .expect("read through link");
    let listing = backend.read_dir(&link).expect("list a linked folder");

    assert_eq!(meta.kind, FileKind::File);
    assert_eq!(contents, b"through");
    assert_eq!(listing, vec![OsString::from("file.txt")]);
}

fn writes_land_inside_a_linked_folder(h: &impl Harness) {
    let backend = h.backend();
    let source = h.root().join("source");
    let link = h.root().join("link");
    backend.create_dir_all(&source).expect("create");
    h.symlink(&link, &source);

    backend
        .write_file(&link.join("written.txt"), b"x")
        .expect("write through link");
    backend
        .create_dir_all(&link.join("made/deep"))
        .expect("create through link");

    let names = sorted(backend.read_dir(&source).expect("list target"));
    let expected: Vec<OsString> = ["made", "written.txt"].map(OsString::from).into();
    assert_eq!(names, expected);
    assert_eq!(
        backend.lstat(&link).expect("lstat link").kind,
        FileKind::Symlink
    );
}

fn writing_through_a_dangling_link_creates_its_target(h: &impl Harness) {
    let backend = h.backend();
    let link = h.root().join("dangling");
    h.symlink(&link, Path::new("made.txt"));

    backend.write_file(&link, b"created").expect("write");

    assert_eq!(
        backend.lstat(&link).expect("lstat link").kind,
        FileKind::Symlink
    );
    assert_eq!(
        backend
            .read_file(&h.root().join("made.txt"))
            .expect("read target"),
        b"created"
    );
}

fn nothing_can_sit_below_a_file(h: &impl Harness) {
    let backend = h.backend();
    let file = h.root().join("file");
    backend.write_file(&file, b"x").expect("write");

    let inspected = backend.lstat(&file.join("x"));
    let written = backend.write_file(&file.join("x"), b"y");
    let created = backend.create_dir_all(&file.join("x"));

    assert_eq!(error_kind(inspected), ErrorKind::NotADirectory);
    assert_eq!(error_kind(written), ErrorKind::NotADirectory);
    assert_eq!(error_kind(created), ErrorKind::NotADirectory);
}

fn removing_a_folder_as_a_file_is_refused(h: &impl Harness) {
    let dir = h.root().join("dir");
    h.backend().create_dir_all(&dir).expect("create");

    let refused = h.backend().remove_file(&dir);

    // macOS `unlink(2)` on a folder fails with `EPERM`.
    assert_eq!(error_kind(refused), ErrorKind::PermissionDenied);
    assert_eq!(
        h.backend().lstat(&dir).expect("still there").kind,
        FileKind::Dir
    );
}

/// `ELOOP` on macOS. Its `ErrorKind` is not nameable on stable Rust.
const ELOOP: i32 = 62;

fn a_symlink_loop_fails_to_resolve(h: &impl Harness) {
    let backend = h.backend();
    h.symlink(&h.root().join("a"), Path::new("b"));
    h.symlink(&h.root().join("b"), Path::new("a"));

    let inspected = backend.lstat(&h.root().join("a/x"));
    let read = backend.read_file(&h.root().join("a"));

    for result in [inspected.map(drop), read.map(drop)] {
        let error = result.expect_err("a loop never resolves");
        assert_eq!(error.raw_os_error(), Some(ELOOP), "got {error:?}");
    }
}

fn gate_refuses_own_data_through_a_symlink(h: &impl Harness) {
    let backend = h.backend();
    let victim = h.root().join("victim");
    let packs = h.root().join("data/rosie/packs");
    backend.create_dir_all(&victim).expect("create victim");
    backend
        .write_file(&victim.join("precious.txt"), b"keep")
        .expect("write victim");
    backend.create_dir_all(&packs).expect("create packs");
    h.symlink(&packs.join("evil"), &victim);
    let gate = Gate::new(backend, empty_bounds(h)).expect("gate");

    let deleted = gate.delete_own(&packs.join("evil/precious.txt"));
    let written = gate.write_own_file(&packs.join("evil/planted.txt"), b"x");

    assert!(matches!(deleted, Err(Error::Symlink { .. })), "{deleted:?}");
    assert!(matches!(written, Err(Error::Symlink { .. })), "{written:?}");
    assert_eq!(
        sorted(backend.read_dir(&victim).expect("list victim")),
        vec![OsString::from("precious.txt")]
    );
}

// A path ending in `/` or `/.` names a folder: the kernel follows a final link and
// refuses any other kind of entry.

fn a_trailing_slash_or_dot_follows_the_final_link_and_demands_a_folder(h: &impl Harness) {
    let backend = h.backend();
    let file = h.root().join("file");
    let target = h.root().join("target");
    let link = h.root().join("link");
    backend.write_file(&file, b"x").expect("write");
    backend.create_dir_all(&target).expect("create");
    h.symlink(&link, Path::new("target"));
    let file_slash = h.root().join("file/");
    let file_dot = file.join(".");

    let not_folders = [
        backend.lstat(&file_slash).map(drop),
        backend.lstat(&file_dot).map(drop),
        backend.read_file(&file_slash).map(drop),
        backend.write_file(&file_slash, b"y"),
        backend.create_dir_all(&file_slash),
        backend.remove_file(&file_slash),
        backend.set_mode(&file_slash, 0o600),
    ];
    let link_kind = backend.lstat(&h.root().join("link/")).map(|meta| meta.kind);
    let link_dot_kind = backend.lstat(&link.join(".")).map(|meta| meta.kind);
    let read_link = backend.read_link(&h.root().join("link/"));
    let new_file = backend.write_file(&h.root().join("new/"), b"y");
    let unlinked_folder = backend.remove_file(&h.root().join("link/"));
    backend
        .remove_empty_dir(&h.root().join("link/"))
        .expect("remove the link's target");

    for result in not_folders {
        assert_eq!(error_kind(result), ErrorKind::NotADirectory);
    }
    assert_eq!(link_kind.expect("lstat through link"), FileKind::Dir);
    assert_eq!(link_dot_kind.expect("lstat through link"), FileKind::Dir);
    assert_eq!(error_kind(read_link), ErrorKind::InvalidInput);
    assert_eq!(error_kind(new_file), ErrorKind::NotFound);
    assert_eq!(error_kind(unlinked_folder), ErrorKind::PermissionDenied);
    assert_eq!(
        sorted(backend.read_dir(h.root()).expect("list")),
        vec![OsString::from("file"), OsString::from("link")]
    );
}

// Real-only: the fake does not model Unicode normalization.

#[test]
fn real_gate_refuses_a_unicode_form_mismatch_naming_the_disk_spelling() {
    let h = RealHarness::new();
    let decomposed = h.root().join("Cafe\u{301}");
    RealBackend
        .create_dir_all(&decomposed)
        .expect("create NFD folder");
    let gate = Gate::new(RealBackend, empty_bounds(&h)).expect("gate");

    let result = gate.check_typed_path(&h.root().join("Caf\u{e9}"));

    assert!(matches!(result, Err(Error::Misspelled { real, .. }) if real == decomposed));
}

#[test]
fn real_gate_refuses_a_case_mismatch_naming_the_disk_spelling() {
    let h = RealHarness::new();
    let spelled = h.root().join("Code");
    RealBackend.create_dir_all(&spelled).expect("create folder");
    let gate = Gate::new(RealBackend, empty_bounds(&h)).expect("gate");

    let result = gate.check_typed_path(&h.root().join("code"));

    assert!(matches!(result, Err(Error::Misspelled { real, .. }) if real == spelled));
}
