use std::io::ErrorKind;

use super::*;
use crate::fs::ROOT_UID;
use crate::fs::error::RealPath;
use crate::fs::fake::{Call, FakeBackend, USER_UID};

const HOME: &str = "/Users/me";
const CONFIG: &str = "/Users/me/.config/rosie";
const DATA: &str = "/Users/me/.local/share/rosie";

fn bounds(roots: &[&str]) -> Bounds {
    Bounds {
        roots: roots.iter().map(PathBuf::from).collect(),
        paths: Paths::new(Path::new(HOME)).expect("absolute home"),
        user_uid: USER_UID,
    }
}

pub(super) fn ctx<'a>(fake: &'a FakeBackend, roots: &[&str]) -> FsContext<&'a FakeBackend> {
    FsContext::new(fake, bounds(roots)).expect("absolute bounds")
}

pub(super) fn ctx_as_root<'a>(fake: &'a FakeBackend, roots: &[&str]) -> FsContext<&'a FakeBackend> {
    FsContext::new_as_root(fake, bounds(roots)).expect("absolute bounds")
}

pub(super) fn removals(fake: &FakeBackend) -> Vec<Call> {
    fake.calls()
        .into_iter()
        .filter(|call| matches!(call, Call::RemoveFile(_) | Call::RemoveEmptyDir(_)))
        .collect()
}

fn delete(ctx: &FsContext<&FakeBackend>, path: &Path) -> Result<(), Error> {
    ctx.delete(ctx.admit_delete(path)?)
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
    let ctx = ctx(&fake, &["/Users/me/code"]);

    delete(&ctx, path("/Users/me/code/app/node_modules")).expect("delete within root");

    assert!(!fake.exists("/Users/me/code/app/node_modules"));
    assert!(fake.exists("/Users/me/code/app/package.json"));
}

#[test]
fn deletes_a_root_itself() {
    let fake = FakeBackend::new();
    fake.add_file("/Users/me/.npm/_cacache/index", "x");
    let ctx = ctx(&fake, &["/Users/me/.npm/_cacache"]);

    delete(&ctx, path("/Users/me/.npm/_cacache")).expect("a root is within itself");

    assert!(!fake.exists("/Users/me/.npm/_cacache"));
}

#[test]
fn refuses_a_path_outside_every_root() {
    let fake = FakeBackend::new();
    fake.add_file("/Users/me/Documents/thesis.tex", "x");
    let ctx = ctx(&fake, &["/Users/me/code", "/Users/me/Library/Caches"]);

    let result = delete(&ctx, path("/Users/me/Documents"));

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
    let ctx = ctx(&fake, &["/Users/me/code"]);

    let escaped = delete(&ctx, path("/Users/me/code/../Documents"));

    assert!(
        matches!(escaped, Err(Error::OutsideRoots { path }) if path == Path::new("/Users/me/Documents"))
    );
    assert!(fake.exists("/Users/me/Documents/thesis.tex"));
}

#[test]
fn resolves_current_and_parent_components_that_stay_inside() {
    let fake = FakeBackend::new();
    fake.add_file("/Users/me/code/app/target/debug/app", "x");
    let ctx = ctx(&fake, &["/Users/me/code"]);

    delete(&ctx, path("/Users/me/code/./app/src/../target")).expect("stays inside root");

    assert!(!fake.exists("/Users/me/code/app/target"));
}

#[test]
fn compares_roots_by_component_not_by_string_prefix() {
    let fake = FakeBackend::new();
    fake.add_file("/foobar/keep.txt", "x");
    let ctx = ctx(&fake, &["/foo"]);

    let result = delete(&ctx, path("/foobar/keep.txt"));

    assert!(matches!(result, Err(Error::OutsideRoots { .. })));
    assert!(fake.exists("/foobar/keep.txt"));
}

#[test]
fn answers_whether_a_path_lies_within_the_roots_as_delete_decides_it() {
    let fake = FakeBackend::new();
    let ctx = ctx(&fake, &["/Users/me/code", "/Users/me/Library/Caches"]);

    let within = |text| ctx.within_roots(path(text)).expect("absolute path");

    assert!(within("/Users/me/code"));
    assert!(within("/Users/me/code/app/node_modules"));
    assert!(within("/Users/me/Library/Caches/pip"));
    assert!(!within("/Users/me/code2"));
    assert!(!within("/Users/me/code/../Documents"));
    assert!(matches!(
        ctx.within_roots(path("code")),
        Err(Error::Relative { .. })
    ));
}

#[test]
fn refuses_relative_paths() {
    let fake = FakeBackend::new();
    let ctx = ctx(&fake, &["/Users/me/code"]);

    let result = delete(&ctx, path("code/app"));

    assert!(matches!(result, Err(Error::Relative { .. })));
}

// symlinks

#[test]
fn refuses_to_delete_through_a_symlink_component_naming_the_real_path() {
    let fake = FakeBackend::new();
    fake.add_file("/Users/me/projects/app/target/out", "x");
    fake.add_symlink("/Users/me/code/escape", "../projects");
    let ctx = ctx(&fake, &["/Users/me/code"]);

    let result = delete(&ctx, path("/Users/me/code/escape/app/target"));

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
    let ctx = ctx(&fake, &["/Users/me/code"]);

    let result = delete(&ctx, path("/Users/me/code/link"));

    assert!(matches!(result, Err(Error::Symlink { .. })));
    assert!(fake.exists("/Users/me/code/link"));
}

// Every delete entry point refuses an item that is itself a symlink, including the
// root delete.
#[test]
fn every_delete_refuses_an_item_that_is_a_symlink() {
    let fake = FakeBackend::running_as(ROOT_UID);
    fake.add_dir("/Library/real");
    fake.add_symlink("/Library/link", "/Library/real");
    fake.chown_with_ancestors("/Library/link", ROOT_UID);
    fake.add_dir(format!("{DATA}/real"));
    fake.add_symlink(format!("{DATA}/link"), "real");
    let ctx = ctx(&fake, &["/Library"]);
    let root = ctx_as_root(&fake, &["/Library"]);

    let as_user = delete(&ctx, path("/Library/link"));
    let as_root = delete(&root, path("/Library/link"));
    let own = ctx.delete_own(&Path::new(DATA).join("link"));

    for result in [as_user, as_root, own] {
        assert!(
            matches!(&result, Err(Error::Symlink { .. })),
            "expected a symlink refusal, got {result:?}"
        );
    }
    assert!(fake.exists("/Library/link"));
    assert!(fake.exists(format!("{DATA}/link")));
    assert_eq!(removals(&fake), vec![]);
}

#[test]
fn removes_symlinks_inside_a_deleted_folder_as_links() {
    let fake = FakeBackend::new();
    fake.add_file("/Users/me/src/lib/keep.js", "x");
    fake.add_symlink("/Users/me/code/app/node_modules/lib", "/Users/me/src/lib");
    let ctx = ctx(&fake, &["/Users/me/code"]);

    delete(&ctx, path("/Users/me/code/app/node_modules")).expect("delete folder");

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
    let ctx = ctx(&fake, &[]);

    let result = ctx.read_file(path("/Users/me/code/app/Cargo.toml"));

    assert!(
        matches!(result, Err(Error::Symlink { real: RealPath::Resolved(real), .. }) if real == Path::new("/Users/me/secret.toml"))
    );
}

#[test]
fn refuses_to_read_a_file_through_a_symlinked_folder() {
    let fake = FakeBackend::new();
    fake.add_file("/Users/me/other/Cargo.toml", "x");
    fake.add_symlink("/Users/me/code/app/linked", "/Users/me/other");
    let ctx = ctx(&fake, &[]);

    let result = ctx.read_file(path("/Users/me/code/app/linked/Cargo.toml"));

    assert!(
        matches!(result, Err(Error::Symlink { link, .. }) if link == Path::new("/Users/me/code/app/linked"))
    );
}

#[test]
fn refuses_to_lstat_through_or_at_a_symlink() {
    let fake = FakeBackend::new();
    fake.add_file("/Users/me/other/Cargo.toml", "x");
    fake.add_symlink("/Users/me/code/app/linked", "/Users/me/other");
    let ctx = ctx(&fake, &[]);

    let through = ctx.lstat_link_free(path("/Users/me/code/app/linked/Cargo.toml"));
    let at = ctx.lstat_link_free(path("/Users/me/code/app/linked"));
    let plain = ctx.lstat_link_free(path("/Users/me/other/Cargo.toml"));

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
    let ctx = ctx(&fake, &[]);

    let result = ctx.lstat_link_free(path("rel/x"));

    assert!(matches!(result, Err(Error::Relative { .. })));
}

#[test]
fn names_the_fully_real_path_when_a_link_leads_through_another_link() {
    let fake = FakeBackend::new();
    fake.add_dir("/private/tmp");
    fake.add_symlink("/tmp", "private/tmp");
    fake.add_symlink("/Users/me/code/escape", "/tmp");
    let ctx = ctx(&fake, &["/Users/me/code"]);

    let deleted = delete(&ctx, path("/Users/me/code/escape/x"));
    let typed = ctx.check_real(path("/Users/me/code/escape/x"));

    for result in [deleted, typed.map(drop)] {
        assert!(
            matches!(&result, Err(Error::Symlink { real: RealPath::Resolved(real), .. }) if real == Path::new("/private/tmp/x")),
            "expected the fully real path, got {result:?}"
        );
    }
}

#[test]
fn refuses_a_path_through_a_dangling_link_leaving_its_real_path_unresolved() {
    let fake = FakeBackend::new();
    fake.add_dir("/Users/me/code");
    fake.add_symlink("/Users/me/code/link", "/Users/me/gone");
    let ctx = ctx(&fake, &["/Users/me/code"]);

    let through = ctx.check_real(path("/Users/me/code/link/x"));
    let at = ctx.check_real(path("/Users/me/code/link"));

    for result in [through, at] {
        assert!(
            matches!(&result, Err(Error::Symlink { link, real: RealPath::Unresolved(_), .. }) if link == Path::new("/Users/me/code/link")),
            "expected a symlink refusal, got {result:?}"
        );
    }
}

#[test]
fn refuses_a_path_through_a_symlink_loop() {
    let fake = FakeBackend::new();
    fake.add_file("/Users/me/code/keep", "x");
    fake.add_symlink("/Users/me/code/a", "b");
    fake.add_symlink("/Users/me/code/b", "a");
    let ctx = ctx(&fake, &["/Users/me/code"]);

    let result = delete(&ctx, path("/Users/me/code/a/x"));

    assert!(result.is_err(), "expected a refusal, got {result:?}");
    assert_eq!(removals(&fake), vec![]);
}

// deletion

#[test]
fn makes_user_owned_read_only_folders_writable_on_the_way_down() {
    let fake = FakeBackend::new();
    fake.add_file("/Users/me/go/pkg/mod/x@v1/go.mod", "module x");
    fake.chmod("/Users/me/go/pkg/mod/x@v1", 0o555);
    let ctx = ctx(&fake, &["/Users/me/go/pkg/mod"]);

    delete(&ctx, path("/Users/me/go/pkg/mod/x@v1")).expect("read-only module deleted");

    assert!(!fake.exists("/Users/me/go/pkg/mod/x@v1"));
}

#[test]
fn leaves_the_permissions_of_other_users_folders_alone() {
    let fake = FakeBackend::new();
    fake.add_file("/Library/Caches/tool/blob", "x");
    fake.chown("/Library/Caches/tool", ROOT_UID);
    fake.chmod("/Library/Caches/tool", 0o555);
    let ctx = ctx(&fake, &["/Library/Caches"]);

    let result = delete(&ctx, path("/Library/Caches/tool"));

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
    let ctx = ctx(&fake, &["/Users/me/code"]);

    let result = delete(&ctx, path("/Users/me/code/app/target"));

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
    let ctx = ctx(&fake, &["/Users/me/code"]);

    let result = delete(&ctx, path("/Users/me/code/app/build"));

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
    let ctx = ctx(&fake, &["/Users/me"]);
    let root = ctx_as_root(&fake, &["/Users/me"]);

    let as_user = delete(&ctx, path(item));
    let as_root = delete(&root, path(item));

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
    let ctx = ctx(&fake, &[]);

    let result = ctx.delete_own(path(own_item));

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
    let ctx = ctx(&fake, &["/Users/me/code"]);

    let folder = delete(&ctx, path("/Users/me/code/a"));
    let file = delete(&ctx, path("/Users/me/code/b/cloud-file"));

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
    let ctx = ctx(&fake, &[]);

    let result = ctx.delete_own(path("/Users/me/.local/share/rosie/packs/p"));

    assert!(matches!(result, Err(Error::Placeholder { .. })));
    assert!(fake.exists("/Users/me/.local/share/rosie/packs/p/cloud/x"));
}

// typed paths

#[test]
fn accepts_a_typed_path_spelled_as_on_disk() {
    let fake = FakeBackend::new();
    fake.add_dir("/Users/me/code/app");
    let ctx = ctx(&fake, &[]);

    let checked = ctx
        .check_real(path("/Users/me/code/./app"))
        .expect("exact spelling");

    assert_eq!(checked, PathBuf::from("/Users/me/code/app"));
}

#[test]
fn refuses_a_case_mismatch_naming_the_correct_spelling() {
    let fake = FakeBackend::new();
    fake.add_dir("/Users/me/Code/App");
    let ctx = ctx(&fake, &[]);

    let result = ctx.check_real(path("/users/me/code/App"));

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
    let ctx = ctx(&fake, &[]);

    let result = ctx.check_real(path("/var/folders/x"));

    assert!(
        matches!(result, Err(Error::Symlink { real: RealPath::Resolved(real), .. }) if real == Path::new("/private/var/folders/x"))
    );
}

#[test]
fn accepts_a_missing_path_below_its_real_deepest_ancestor() {
    let fake = FakeBackend::new();
    fake.add_dir("/Users/me");
    let ctx = ctx(&fake, &[]);

    let result = ctx.check_real(path("/Users/me/nope/deeper"));

    assert_eq!(
        result.expect("nothing missing can be a link"),
        PathBuf::from("/Users/me/nope/deeper")
    );
}

// rosie's own data

#[test]
fn writes_own_data_without_any_roots() {
    let fake = FakeBackend::new();
    fake.add_dir(HOME);
    let ctx = ctx(&fake, &[]);
    let config = Path::new(CONFIG).join("config.toml");
    let pack = Path::new(DATA).join("packs/builtin");

    ctx.create_own_dir_all(Path::new(CONFIG))
        .expect("create config folder");
    ctx.write_own_file(&config, b"roots = []")
        .expect("write config");
    ctx.create_own_dir_all(&pack).expect("create pack folder");
    ctx.delete_own(&pack).expect("remove pack");

    assert_eq!(
        ctx.read_own_file(&config).expect("read config"),
        b"roots = []"
    );
    assert!(!fake.exists(&pack));
}

#[test]
fn confines_own_data_to_rosies_folders() {
    let fake = FakeBackend::new();
    fake.add_file("/Users/me/.zshrc", "x");
    let ctx = ctx(&fake, &[HOME]);

    let outside = ctx.write_own_file(path("/Users/me/.zshrc"), b"pwned");
    let escaped = ctx.delete_own(path("/Users/me/.local/share/rosie/../../../.zshrc"));
    let prefixed = ctx.write_own_file(path("/Users/me/.config/rosie-evil/x"), b"x");
    let created = ctx.create_own_file(path("/Users/me/.zshrc"), b"pwned");

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
    let ctx = ctx(&fake, &[]);
    let evil = Path::new(DATA).join("packs/evil");
    let real = Path::new(DATA).join("packs/real");

    let results = [
        ctx.read_own_file(&evil.join("precious.txt")).map(drop),
        ctx.write_own_file(&evil.join("planted.txt"), b"x"),
        ctx.create_own_file(&evil.join("planted.txt"), b"x"),
        ctx.create_own_dir_all(&evil.join("sub")),
        ctx.rename_own(&evil, &real.join("moved")),
        ctx.rename_own(&real, &evil.join("moved")),
        ctx.delete_own(&evil.join("precious.txt")),
        ctx.delete_own(&evil),
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
    let ctx = ctx(&fake, &[]);
    let config = Path::new(CONFIG).join("config.toml");

    let read = ctx.read_own_file(&config);
    let written = ctx.write_own_file(&config, b"roots = [\"/\"]");

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
    let ctx = ctx(&fake, &[]);

    let result = ctx.delete_own(path("/Users/me/.local/share/rosie/."));

    assert!(matches!(result, Err(Error::OwnFolderItself { path }) if path == Path::new(DATA)));
    assert!(fake.exists("/Users/me/.local/share/rosie/packs/builtin/pack.toml"));
}
