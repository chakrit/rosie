//! What an interrupted or failed pull, remove, or first-run check leaves beside a pack,
//! and how the next run settles it (`docs/spec/rules.md#packs`). The states and their
//! transitions are listed on `rosie::packs::Store`.
//!
//! A crash is modeled by building the state it leaves in a fresh fake; a failure is an
//! injected error. A pack may only come back from `.<repo>.old`, the only copy of a pack
//! a swap set aside; a removed pack must never come back.

mod pack_fixture;
mod tarball;

use std::io::ErrorKind;
use std::path::Path;

use pack_fixture::{
    Canned, DATA, OWNER, ROSIE_PACK, ROSIE_REMOVED, ROSIE_SET_ASIDE, ROSIE_STAGING, ROSIE_URL, at,
    contents, fake_home, gate, listing,
};
use rosie::fs::Op;
use rosie::fs::fake::FakeBackend;
use rosie::packs::{self, Error, FirstRun, Installed, RemovePack, Source, Store};
use tarball::Tarball;

const V1: &str = "# version 1";
const V2: &str = "# version 2";

/// A complete pack copy at `dir`: one rule file and its pull time.
fn add_copy(fake: &FakeBackend, dir: &str, rule: &str, pulled_at: u64) {
    fake.add_file(format!("{dir}/node.toml"), rule);
    fake.add_file(format!("{dir}/.pulled"), pulled_at.to_string());
}

/// A copy an interrupted delete left: the fake deletes `.pulled` first, so only the
/// rule file remains.
fn add_partial_copy(fake: &FakeBackend, dir: &str, rule: &str) {
    fake.add_file(format!("{dir}/node.toml"), rule);
}

fn remove_rosie<B: rosie::fs::Backend>(store: &Store<B>) -> Result<Source, Error> {
    RemovePack {
        pack: "rosie".to_owned(),
    }
    .run(store)
}

fn rosie_pulled_at(seconds: u64) -> Vec<Installed> {
    vec![Installed {
        source: Source::default(),
        pulled_at: at(seconds),
    }]
}

fn assert_no_copies_beside_the_pack(fake: &FakeBackend) {
    for copy in [ROSIE_STAGING, ROSIE_SET_ASIDE, ROSIE_REMOVED] {
        assert!(!fake.exists(copy), "{copy} is left behind");
    }
}

fn assert_offline_first_run_pulls<B: rosie::fs::Backend>(store: &Store<B>) {
    let result = packs::first_run(store, &Canned::offline(), at(9));

    assert!(
        matches!(&result, Err(Error::Download { url, .. }) if url == ROSIE_URL),
        "the next run must pull, not bring a pack back: {result:?}"
    );
    assert_eq!(store.installed().expect("list packs"), vec![]);
}

// pull: crash while staging

#[test]
fn a_staging_copy_an_interrupted_pull_left_is_cleared_and_the_pack_kept() {
    let fake = fake_home();
    add_copy(&fake, ROSIE_PACK, V1, 1);
    add_partial_copy(&fake, ROSIE_STAGING, V2);
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));

    let outcome = packs::first_run(&store, &Canned::offline(), at(9)).expect("ready");

    assert!(matches!(outcome, FirstRun::Ready), "got {outcome:?}");
    assert_eq!(contents(&fake, format!("{ROSIE_PACK}/node.toml")), V1);
    assert_no_copies_beside_the_pack(&fake);
}

#[test]
fn a_staging_copy_an_interrupted_first_pull_left_is_never_installed() {
    let fake = fake_home();
    add_copy(&fake, ROSIE_STAGING, V1, 1);
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));

    assert_offline_first_run_pulls(&store);
    assert_no_copies_beside_the_pack(&fake);
}

// pull: crash between setting the old pack aside and moving the new one in

#[test]
fn first_run_puts_back_a_pack_an_interrupted_swap_set_aside() {
    let fake = fake_home();
    add_copy(&fake, ROSIE_SET_ASIDE, V1, 1);
    add_copy(&fake, ROSIE_STAGING, V2, 2);
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
    let network = Canned::offline();

    let outcome = packs::first_run(&store, &network, at(9)).expect("ready offline");

    assert!(matches!(outcome, FirstRun::Ready), "got {outcome:?}");
    assert_eq!(network.requested(), Vec::<String>::new());
    assert_eq!(contents(&fake, format!("{ROSIE_PACK}/node.toml")), V1);
    assert_eq!(store.installed().expect("list packs"), rosie_pulled_at(1));
    assert_no_copies_beside_the_pack(&fake);
}

#[test]
fn a_failed_pull_puts_back_a_pack_an_interrupted_swap_set_aside() {
    let fake = fake_home();
    add_copy(&fake, ROSIE_SET_ASIDE, V1, 1);
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));

    let result = packs::pull(&store, &Canned::offline(), &Source::default(), at(9));

    assert!(
        matches!(result, Err(Error::Download { .. })),
        "got {result:?}"
    );
    assert_eq!(contents(&fake, format!("{ROSIE_PACK}/node.toml")), V1);
    assert_eq!(listing(&fake, OWNER), vec!["rosie"]);
}

#[test]
fn removing_a_pack_an_interrupted_swap_set_aside_removes_it() {
    let fake = fake_home();
    add_copy(&fake, ROSIE_SET_ASIDE, V1, 1);
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));

    let removed = remove_rosie(&store).expect("the set-aside pack is still installed");

    assert_eq!(removed, Source::default());
    assert_no_copies_beside_the_pack(&fake);
    assert_offline_first_run_pulls(&store);
}

// pull: the new pack moves in but the old one cannot be put back

#[test]
fn a_failed_swap_whose_restore_also_fails_names_the_set_aside_copy() {
    let fake = fake_home();
    add_copy(&fake, ROSIE_PACK, V1, 1);
    fake.fail_on(ROSIE_STAGING, Op::Rename, ErrorKind::PermissionDenied);
    fake.fail_on(ROSIE_SET_ASIDE, Op::Rename, ErrorKind::PermissionDenied);
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", V2)]).gzip();

    let result = packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, tarball),
        &Source::default(),
        at(2),
    );

    assert!(
        matches!(&result, Err(Error::Restore { set_aside, .. }) if set_aside == Path::new(ROSIE_SET_ASIDE)),
        "got {result:?}"
    );
    assert_eq!(contents(&fake, format!("{ROSIE_SET_ASIDE}/node.toml")), V1);
}

// pull: crash after a good swap, before the old pack is discarded

#[test]
fn a_stale_copy_beside_a_newer_pack_is_discarded_not_restored() {
    let fake = fake_home();
    add_copy(&fake, ROSIE_PACK, V2, 2);
    add_copy(&fake, ROSIE_SET_ASIDE, V1, 1);
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));

    let outcome = packs::first_run(&store, &Canned::offline(), at(9)).expect("ready");

    assert!(matches!(outcome, FirstRun::Ready), "got {outcome:?}");
    assert_eq!(store.installed().expect("list packs"), rosie_pulled_at(2));
    assert_eq!(contents(&fake, format!("{ROSIE_PACK}/node.toml")), V2);
    assert_no_copies_beside_the_pack(&fake);
}

#[test]
fn removing_a_pack_with_a_stale_copy_beside_it_does_not_resurrect_the_copy() {
    let fake = fake_home();
    add_copy(&fake, ROSIE_PACK, V2, 2);
    add_copy(&fake, ROSIE_SET_ASIDE, V1, 1);
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));

    remove_rosie(&store).expect("remove rosie");

    assert_no_copies_beside_the_pack(&fake);
    assert_offline_first_run_pulls(&store);
}

// pull: discarding the old pack after a good swap fails partway

#[test]
fn a_failed_discard_after_a_good_swap_leaves_nothing_restorable() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
    let first = Tarball::pack(&[("node.toml", V1)]).gzip();
    packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, first),
        &Source::default(),
        at(1),
    )
    .expect("first pull");
    // Whatever name the old copy is deleted under, deleting its rule file fails.
    for copy in [ROSIE_SET_ASIDE, ROSIE_REMOVED] {
        fake.fail_on(
            format!("{copy}/node.toml"),
            Op::RemoveFile,
            ErrorKind::PermissionDenied,
        );
    }
    let second = Tarball::pack(&[("node.toml", V2)]).gzip();

    let result = packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, second),
        &Source::default(),
        at(2),
    );

    assert!(result.is_err(), "the failed delete is reported: {result:?}");
    assert_eq!(store.installed().expect("list packs"), rosie_pulled_at(2));
    assert!(
        !fake.exists(ROSIE_SET_ASIDE),
        "the old copy must not wait under the restorable name"
    );
}

#[test]
fn a_partly_discarded_copy_beside_a_newer_pack_is_cleared() {
    let fake = fake_home();
    add_copy(&fake, ROSIE_PACK, V2, 2);
    add_partial_copy(&fake, ROSIE_REMOVED, V1);
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));

    let outcome = packs::first_run(&store, &Canned::offline(), at(9)).expect("ready");

    assert!(matches!(outcome, FirstRun::Ready), "got {outcome:?}");
    assert_eq!(store.installed().expect("list packs"), rosie_pulled_at(2));
    assert_no_copies_beside_the_pack(&fake);
}

#[test]
fn removing_a_pack_with_a_partly_discarded_copy_beside_it_removes_both() {
    let fake = fake_home();
    add_copy(&fake, ROSIE_PACK, V2, 2);
    add_partial_copy(&fake, ROSIE_REMOVED, V1);
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));

    remove_rosie(&store).expect("remove rosie");

    assert_no_copies_beside_the_pack(&fake);
    assert_offline_first_run_pulls(&store);
}

// remove: crash after the pack is renamed aside, before or during its delete

#[test]
fn a_removed_copy_an_interrupted_remove_left_is_never_installed() {
    let fake = fake_home();
    add_copy(&fake, ROSIE_REMOVED, V1, 1);
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));

    assert_offline_first_run_pulls(&store);
    assert_no_copies_beside_the_pack(&fake);
}

#[test]
fn an_interrupted_remove_does_not_resurrect_the_pack() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", V1)]).gzip();
    packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, tarball),
        &Source::default(),
        at(1),
    )
    .expect("pull");
    fake.fail_on(
        format!("{ROSIE_REMOVED}/node.toml"),
        Op::RemoveFile,
        ErrorKind::PermissionDenied,
    );

    let result = remove_rosie(&store);

    assert!(result.is_err(), "the failed delete is reported: {result:?}");
    assert!(!fake.exists(ROSIE_PACK), "the pack folder stays gone");
    assert!(!fake.exists(ROSIE_SET_ASIDE));
    assert_eq!(store.installed().expect("list packs"), vec![]);
}

// remove: crash after the delete, before the empty owner folder goes

#[test]
fn an_empty_owner_folder_an_interrupted_remove_left_is_not_a_pack() {
    let fake = fake_home();
    fake.add_dir(OWNER);
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", V1)]).gzip();

    let outcome = packs::first_run(&store, &Canned::serving(ROSIE_URL, tarball), at(9))
        .expect("first run pulls");

    assert!(matches!(outcome, FirstRun::Pulled(_)), "got {outcome:?}");
    assert_eq!(store.installed().expect("list packs"), rosie_pulled_at(9));
}
