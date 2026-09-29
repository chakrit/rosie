//! The scanned app's own bundle: how it is identified and named, and what refuses it.

use super::*;
use crate::fs::fake::Call;
use crate::fs::{CommandOutput, Exit};
use crate::process::{self, ProcessTable};

/// Bar.app whose `Info.plist` answers `plutil` with `id` and `name` for the two keys.
fn bar_app_answering(fake: &FakeBackend, id: CommandOutput, name: CommandOutput) {
    let plist = info_plist(Path::new(BAR));
    fake.add_file(&plist, "<plist/>");
    fake.add_sized_file(format!("{BAR}/Contents/MacOS/app"), 1 << 20);
    fake.respond(extract_argv(&plist, "CFBundleIdentifier"), id);
    fake.respond(extract_argv(&plist, "CFBundleName"), name);
    quiet(fake);
}

fn app_scan(fake: &FakeBackend, app: &str) -> Result<Scanned, Error> {
    scan_app(
        &gate(fake, &ALL_ROOTS),
        Path::new(app),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    )
}

#[test]
fn a_scanned_app_whose_bundle_id_cannot_be_read_is_refused() {
    let fake = FakeBackend::new();
    bar_app_answering(
        &fake,
        failed("Bar.app/Contents/Info.plist: Unexpected character\n"),
        succeeded("Bar\n"),
    );

    let result = app_scan(&fake, BAR);

    assert!(
        matches!(result, Err(Error::Process(process::Error::Failed { .. }))),
        "{result:?}"
    );
}

#[test]
fn a_scanned_app_whose_bundle_name_cannot_be_read_is_refused() {
    let fake = FakeBackend::new();
    bar_app_answering(
        &fake,
        succeeded("com.foo.Bar\n"),
        failed("Bar.app/Contents/Info.plist: Unexpected character\n"),
    );

    let result = app_scan(&fake, BAR);

    assert!(
        matches!(result, Err(Error::Process(process::Error::Failed { .. }))),
        "{result:?}"
    );
}

/// `plutil` ends a raw value with a newline; a value printed without one is still read
/// whole.
#[test]
fn a_plist_value_printed_without_a_newline_is_read_whole() {
    let fake = FakeBackend::new();
    bar_app_answering(&fake, succeeded("com.foo.Bar"), succeeded("Bar"));

    let scanned = app_scan(&fake, BAR).expect("app scan");

    assert_eq!(entry(&scanned, BAR).rules, ["app/com.foo.Bar"]);
}

#[test]
fn reports_name_the_app_by_its_bundle_name_else_its_file_name() {
    let named = FakeBackend::new();
    bar_app_answering(&named, succeeded("com.foo.Bar\n"), succeeded("Bar Pro\n"));
    named.add_dir(format!("{BAR}/Contents/Library/LoginItems/BarHelper.app"));
    let unnamed = FakeBackend::new();
    bar_app_answering(
        &unnamed,
        succeeded("com.foo.Bar\n"),
        no_value(&info_plist(Path::new(BAR)), "CFBundleName"),
    );
    unnamed.add_dir(format!("{BAR}/Contents/Library/LoginItems/BarHelper.app"));

    let named = app_scan(&named, BAR).expect("app scan");
    let unnamed = app_scan(&unnamed, BAR).expect("app scan");

    assert_eq!(
        named.plan.reports()[0].subject(),
        "login item BarHelper.app of Bar Pro"
    );
    assert_eq!(
        unnamed.plan.reports()[0].subject(),
        "login item BarHelper.app of Bar"
    );
}

#[test]
fn refuses_a_file_named_like_an_app() {
    let fake = FakeBackend::new();
    fake.add_file("/Users/me/Bar.app", "x");
    quiet(&fake);

    let result = app_scan(&fake, "/Users/me/Bar.app");

    assert!(matches!(result, Err(Error::NotAnApp { .. })), "{result:?}");
}

/// App mode never crosses volumes (`docs/spec/safety.md#walk-skips`), and a bundle
/// that is a volume root would plan that whole volume.
#[test]
fn refuses_a_bundle_that_is_a_mounted_volume_before_reading_or_sizing_it() {
    let fake = FakeBackend::new();
    install_app(&fake, BAR, "com.foo.Bar", Some("Bar"));
    quiet(&fake);
    fake.mount(BAR, 7);

    let result = app_scan(&fake, BAR);

    assert!(
        matches!(&result, Err(Error::Mount { path }) if path == Path::new(BAR)),
        "{result:?}"
    );
    assert!(
        !fake.calls().iter().any(|call| matches!(
            call,
            Call::ReadDir(path) | Call::ReadFile(path) if path.starts_with(BAR)
        ) || matches!(call, Call::Run(_))),
        "{:?}",
        fake.calls()
    );
}

// system commands

#[test]
fn a_system_command_that_cannot_be_run_fails_the_scan() {
    let fake = FakeBackend::new();
    install_app(&fake, BAR, "com.foo.Bar", Some("Bar"));
    running(&fake, "    1     0     0 /sbin/launchd\n");

    let result = app_scan(&fake, BAR);

    assert!(
        matches!(
            result,
            Err(Error::Process(process::Error::Fs(
                fs::Error::Command { .. }
            )))
        ),
        "{result:?}"
    );
}

#[test]
fn system_command_output_that_is_not_utf8_fails_the_scan() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    let mut listing = b"com.foo.Bar.pkg\n".to_vec();
    listing.push(0xff);
    fake.respond(
        pkgs_argv(),
        CommandOutput {
            exit: Exit::Code(0),
            stdout: listing,
            stderr: Vec::new(),
        },
    );

    let result = app_scan(&fake, BAR);

    assert!(
        matches!(result, Err(Error::Process(process::Error::NotUtf8 { .. }))),
        "{result:?}"
    );
}

#[test]
fn a_failing_process_listing_fails_the_scan() {
    let fake = FakeBackend::new();
    install_app(&fake, BAR, "com.foo.Bar", Some("Bar"));
    receipts(&fake, "com.apple.pkg.Core\n");
    fake.respond(ProcessTable::argv(), failed("ps: broken\n"));

    let result = app_scan(&fake, BAR);

    assert!(
        matches!(result, Err(Error::Process(process::Error::Failed { .. }))),
        "{result:?}"
    );
}
