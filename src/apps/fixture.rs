//! Test fixtures for app and orphan scans: apps installed in the fake, with canned
//! `plutil`, `lsof`, `pkgutil`, and `launchctl` output.

use std::path::{Path, PathBuf};

use super::app::pkgs_argv;
use super::bundle::{extract_argv, info_plist};
use super::claims::{launch_jobs_argv, system_jobs_argv};
use crate::config::Walk;
use crate::fs::fake::{FakeBackend, USER_UID};
use crate::fs::{CommandOutput, Exit, FsContext, Paths};
use crate::plan::AggressiveItems;
use crate::scan::Settings;

pub(super) const HOME: &str = "/Users/me";

/// Every leftover location, `/Applications`, and `~/Applications` as a root.
pub(super) const ALL_ROOTS: [&str; 5] = [
    "/Users/me/Library",
    "/Users/me/Applications",
    "/Library/LaunchDaemons",
    "/Library/PrivilegedHelperTools",
    "/Applications",
];

pub(super) fn ctx<'a>(fake: &'a FakeBackend, roots: &[&str]) -> FsContext<&'a FakeBackend> {
    ctx_at_home(fake, roots, HOME)
}

/// A context for the user whose home is `home`.
pub(super) fn ctx_at_home<'a>(
    fake: &'a FakeBackend,
    roots: &[&str],
    home: &str,
) -> FsContext<&'a FakeBackend> {
    let paths = Paths::new(Path::new(home)).expect("absolute home");
    let roots = roots.iter().map(PathBuf::from).collect();
    FsContext::new(fake, paths.bounds(roots, USER_UID)).expect("absolute bounds")
}

/// The settings of a scan with the default walk.
pub(super) fn settings(aggressive: AggressiveItems) -> Settings {
    Settings {
        walk: Walk::default(),
        aggressive,
    }
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

/// What `launchctl list` prints: its header, then `rows` of `<pid>\t<status>\t<label>`.
pub(super) fn loaded(fake: &FakeBackend, rows: &str) {
    let listing = format!("PID\tStatus\tLabel\n{rows}");
    fake.respond(launch_jobs_argv(), succeeded(&listing));
}

/// What `launchctl print system` prints, trimmed to its shape: a `services` block whose
/// `rows` are `<pid> <status> <label>`, then a `disabled services` block naming
/// `com.disabled.Daemon`, which is not loaded.
pub(super) fn loaded_system(fake: &FakeBackend, rows: &str) {
    let services: String = rows.lines().map(|row| format!("\t\t{row}\n")).collect();
    let printed = format!(
        "system = {{\n\ttype = system\n\n\tservices = {{\n{services}\t}}\n\n\
         \tdisabled services = {{\n\t\t\"com.disabled.Daemon\" => disabled\n\t}}\n}}\n"
    );
    fake.respond(system_jobs_argv(), succeeded(&printed));
}

pub(super) fn receipts(fake: &FakeBackend, pkgs: &str) {
    fake.respond(pkgs_argv(), succeeded(pkgs));
}

/// Canned answers for a scan that finds no running program but launchd, no receipt,
/// and no loaded launch job but launchd's own.
pub(super) fn quiet(fake: &FakeBackend) {
    fake.respond_running(&[(1, "/sbin/launchd")]);
    receipts(fake, "com.apple.pkg.CLTools_SDK_macOS13\n");
    loaded(fake, "1\t0\tcom.apple.launchd\n");
    loaded_system(fake, "1 - com.apple.launchd.system");
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
