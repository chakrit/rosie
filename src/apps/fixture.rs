//! Test fixtures for app and orphan scans: apps installed in the fake, with canned
//! `plutil`, `ps`, and `pkgutil` output.

use std::path::{Path, PathBuf};

use super::app::pkgs_argv;
use super::bundle::{extract_argv, info_plist};
use crate::fs::fake::{FakeBackend, USER_UID};
use crate::fs::{Bounds, CommandOutput, Exit, Gate};
use crate::process::ProcessTable;

pub(super) const HOME: &str = "/Users/me";

/// Every leftover location and app folder as a root.
pub(super) const ALL_ROOTS: [&str; 5] = [
    "/Users/me/Library",
    "/Users/me/Applications",
    "/Library/LaunchDaemons",
    "/Library/PrivilegedHelperTools",
    "/Applications",
];

pub(super) fn gate<'a>(fake: &'a FakeBackend, roots: &[&str]) -> Gate<&'a FakeBackend> {
    let bounds = Bounds {
        roots: roots.iter().map(PathBuf::from).collect(),
        config_dir: PathBuf::from("/Users/me/.config/rosie"),
        data_dir: PathBuf::from("/Users/me/.local/share/rosie"),
        user_uid: USER_UID,
    };
    Gate::new(fake, bounds).expect("absolute bounds")
}

pub(super) fn home() -> &'static Path {
    Path::new(HOME)
}

pub(super) fn succeeded(stdout: &str) -> CommandOutput {
    CommandOutput {
        exit: Exit::Code(0),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

pub(super) fn failed(stderr: &str) -> CommandOutput {
    CommandOutput {
        exit: Exit::Code(1),
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

/// What `plutil -extract` prints for a key the plist does not hold.
pub(super) fn no_value(plist: &Path, key: &str) -> CommandOutput {
    failed(&format!(
        "{}: Could not extract value, error: No value at that key path or invalid key path: {key}\n",
        plist.display()
    ))
}

/// Installs a bundle with an `Info.plist` whose `plutil` answers are canned.
pub(super) fn install_app(fake: &FakeBackend, bundle: &str, id: &str, name: Option<&str>) {
    let plist = info_plist(Path::new(bundle));
    fake.add_file(&plist, "<plist/>");
    fake.add_sized_file(Path::new(bundle).join("Contents/MacOS/app"), 1 << 20);

    fake.respond(
        extract_argv(&plist, "CFBundleIdentifier"),
        succeeded(&format!("{id}\n")),
    );
    let name_output = match name {
        Some(name) => succeeded(&format!("{name}\n")),
        None => no_value(&plist, "CFBundleName"),
    };
    fake.respond(extract_argv(&plist, "CFBundleName"), name_output);
}

pub(super) fn running(fake: &FakeBackend, ps: &str) {
    fake.respond(ProcessTable::argv(), succeeded(ps));
}

pub(super) fn receipts(fake: &FakeBackend, pkgs: &str) {
    fake.respond(pkgs_argv(), succeeded(pkgs));
}

/// Canned answers for an app scan that finds no process and no receipt.
pub(super) fn quiet(fake: &FakeBackend) {
    running(fake, "    1     0     0 /sbin/launchd\n");
    receipts(fake, "com.apple.pkg.CLTools_SDK_macOS13\n");
}

/// Installs an iPhone or iPad app as macOS on Apple silicon does: the real bundle in
/// `Wrapper/<name>.app`, reached also through a `WrappedBundle` symlink, and no
/// `Contents/`. Only the wrapped `Info.plist` has canned `plutil` answers.
pub(super) fn install_wrapped_app(fake: &FakeBackend, bundle: &str, id: &str, name: &str) {
    let wrapped = Path::new(bundle).join(format!("Wrapper/{name}.app"));
    let plist = wrapped.join("Info.plist");
    fake.add_file(&plist, "<plist/>");
    fake.add_sized_file(wrapped.join(name), 1 << 20);
    fake.add_file(Path::new(bundle).join("iTunesMetadata.plist"), "<plist/>");
    fake.add_symlink(Path::new(bundle).join("WrappedBundle"), &wrapped);

    fake.respond(
        extract_argv(&plist, "CFBundleIdentifier"),
        succeeded(&format!("{id}\n")),
    );
    fake.respond(
        extract_argv(&plist, "CFBundleName"),
        succeeded(&format!("{name}\n")),
    );
}
