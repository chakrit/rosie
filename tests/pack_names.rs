//! Pack names the volume folds together, against `RealBackend` in a temp folder. The
//! fake folds case only, so the Unicode-form and APFS case-folding cases run here.

mod pack_fixture;
mod tarball;

use std::path::{Path, PathBuf};

use pack_fixture::{Canned, NODE_RULE, at, source, url_of};
use rosie::fs::{Backend, Bounds, Gate, RealBackend};
use rosie::packs::{self, Error, Source, Store};
use tarball::Tarball;
use tempfile::TempDir;

/// A symlink-free temp folder holding rosie's own folders.
struct RealHome {
    _temp: TempDir,
    root: PathBuf,
}

impl RealHome {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("create temp folder");
        // macOS temp folders sit under the `/var` → `/private/var` link.
        let root = temp
            .path()
            .canonicalize()
            .expect("canonicalize temp folder");
        RealHome { _temp: temp, root }
    }

    fn data(&self) -> PathBuf {
        self.root.join("data/rosie")
    }

    fn gate(&self) -> Gate<RealBackend> {
        let user_uid = RealBackend
            .lstat(&self.root)
            .expect("lstat temp folder")
            .uid;
        let bounds = Bounds {
            roots: vec![],
            config_dir: self.root.join("config/rosie"),
            data_dir: self.data(),
            user_uid,
        };
        Gate::new(RealBackend, bounds).expect("absolute bounds")
    }
}

#[test]
fn pull_refuses_a_pack_name_installed_under_another_unicode_form() {
    let home = RealHome::new();
    let gate = home.gate();
    let store = Store::new(&gate, &home.data());
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let composed = "someone/donn\u{e9}es";
    let decomposed = "other/donne\u{301}es";
    let network =
        Canned::serving(&url_of(composed), tarball.clone()).and(&url_of(decomposed), tarball);
    packs::pull(&store, &network, &source(composed), at(1)).expect("first pull");

    let result = packs::pull(&store, &network, &source(decomposed), at(2));

    assert!(
        matches!(&result, Err(Error::PackNameTaken { installed, .. }) if *installed == source(composed)),
        "got {result:?}"
    );
    assert!(!home.data().join("packs/other").exists());
}

#[test]
fn pull_installs_under_an_owner_stored_in_another_unicode_form() {
    let home = RealHome::new();
    let gate = home.gate();
    let store = Store::new(&gate, &home.data());
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let composed = "donn\u{e9}es/rosie";
    let decomposed = "donne\u{301}es/rosie";
    let network =
        Canned::serving(&url_of(composed), tarball.clone()).and(&url_of(decomposed), tarball);
    packs::pull(&store, &network, &source(composed), at(1)).expect("first pull");

    let pulled = packs::pull(&store, &network, &source(decomposed), at(2)).expect("second pull");

    assert_eq!(pulled.source, source(composed));
    let installed = store.installed().expect("list packs");
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].source, source(composed));
    assert_eq!(installed[0].pulled_at, at(2));
}

/// APFS also folds `ſ` (long s) onto `s`, which no case mapping in std does.
#[test]
fn pull_refuses_a_pack_the_volume_folds_onto_the_reserved_name_user() {
    let home = RealHome::new();
    let gate = home.gate();
    let store = Store::new(&gate, &home.data());
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let respelled = "someone/u\u{17f}er";
    let network = Canned::serving(&url_of(respelled), tarball);

    let result = packs::pull(&store, &network, &source(respelled), at(1));

    assert!(
        matches!(&result, Err(Error::ReservedPack { text }) if text == respelled),
        "got {result:?}"
    );
    assert_eq!(store.sources().expect("list packs"), Vec::<Source>::new());
    let staging = Path::new("packs/someone").join(".u\u{17f}er.new");
    assert!(!home.data().join(staging).exists());
}
