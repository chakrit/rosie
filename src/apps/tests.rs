use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::app::pkgs_argv;
use super::bundle::{extract_argv, info_plist};
use super::fixture::{
    ALL_ROOTS, HOME, failed, gate, home, install_app, install_wrapped_app, no_value, quiet,
    receipts, running, succeeded,
};
use super::*;
use crate::fs::ROOT_UID;
use crate::fs::fake::{FakeBackend, USER_UID};
use crate::fs::{self, Home, Op};
use crate::plan::{AggressiveItems, Delete, LaunchDomain, Needs, RunAs, Size, Status, WalkSkip};
use crate::scan::{Mounts, Scan};

mod app_bundle;
mod app_scan;
mod home_resolution;
mod installed;
mod locations;
mod orphan_scan;
mod symlinks;

const BAR: &str = "/Applications/Bar.app";
const NPLAYER: &str = "/Applications/nPlayer.app";

fn scanned_app(fake: &FakeBackend, roots: &[&str], aggressive: AggressiveItems) -> Scan {
    scan_app(
        &gate(fake, roots),
        Path::new(BAR),
        &home(),
        aggressive,
        Mounts::Skip,
    )
    .expect("app scan")
}

fn scanned_orphans(fake: &FakeBackend, aggressive: AggressiveItems) -> Scan {
    scan_orphans(&gate(fake, &ALL_ROOTS), &home(), aggressive, Mounts::Skip).expect("orphan scan")
}

/// `(path, status)` of every delete entry, in plan order.
fn deletes(scanned: &Scan) -> Vec<(String, Status)> {
    scanned
        .plan
        .deletes()
        .iter()
        .map(|delete| (delete.path.display().to_string(), delete.status))
        .collect()
}

fn entry<'a>(scanned: &'a Scan, path: &str) -> &'a Delete {
    scanned
        .plan
        .deletes()
        .iter()
        .find(|delete| delete.path == Path::new(path))
        .unwrap_or_else(|| panic!("{path} is not in the plan: {:?}", deletes(scanned)))
}

fn lib(rest: &str) -> String {
    format!("{HOME}/Library/{rest}")
}

fn bar_app(fake: &FakeBackend) {
    install_app(fake, BAR, "com.foo.Bar", Some("Bar"));
    quiet(fake);
}

/// Bar in `/Applications`, Baz in a suite folder, Qux in `~/Applications`, and Notes and Sys
/// in `/System/Applications` are installed; only `launchd` runs.
fn installed_suite(fake: &FakeBackend) {
    running(fake, "    1     0     0 /sbin/launchd\n");
    install_app(fake, BAR, "com.foo.Bar", Some("Bar"));
    install_app(
        fake,
        "/Applications/Suite/Baz.app",
        "com.baz.Baz",
        Some("Baz"),
    );
    install_app(
        fake,
        "/Users/me/Applications/Qux.app",
        "com.qux.Qux",
        Some("Qux"),
    );
    install_app(
        fake,
        "/System/Applications/Notes.app",
        "com.apple.Notes",
        Some("Notes"),
    );
    install_app(
        fake,
        "/System/Applications/Sys.app",
        "org.sys.Sys",
        Some("Sys"),
    );
}
