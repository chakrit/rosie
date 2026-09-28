//! What a leftover location holds, and how a failure to look into one surfaces.

use super::*;

#[test]
fn webkit_and_http_storages_are_searched() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_dir(lib("WebKit/com.foo.Bar"));
    fake.add_dir(lib("HTTPStorages/com.foo.Bar"));

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert_eq!(
        deletes(&scanned),
        [
            (BAR.to_owned(), Status::Ticked),
            (lib("HTTPStorages/com.foo.Bar"), Status::Ticked),
            (lib("WebKit/com.foo.Bar"), Status::Ticked),
        ]
    );
}

#[test]
fn a_leftover_location_that_is_a_file_holds_nothing() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_file(lib("Caches"), "x");
    fake.add_file(lib("Preferences/com.foo.Bar.plist"), "x");

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert_eq!(
        deletes(&scanned),
        [
            (BAR.to_owned(), Status::Ticked),
            (lib("Preferences/com.foo.Bar.plist"), Status::Ticked),
        ]
    );
    assert_eq!(scanned.skips.total(), 0);
}

#[test]
fn a_leftover_location_listing_that_fails_other_than_denied_fails_the_scan() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_dir(lib("Caches/com.foo.Bar"));
    fake.fail_on(lib("Caches"), Op::ReadDir, ErrorKind::Other);

    let result = scan_app(
        &gate(&fake, &ALL_ROOTS),
        Path::new(BAR),
        home(),
        AggressiveItems::Unticked,
    );

    assert!(
        matches!(
            result,
            Err(Error::Fs(fs::Error::Io {
                op: Op::ReadDir,
                ..
            }))
        ),
        "{result:?}"
    );
}

#[test]
fn an_entry_rosie_cannot_lstat_fails_the_scan() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_dir(lib("Caches/com.foo.Bar"));
    fake.fail_on(
        lib("Caches/com.foo.Bar"),
        Op::Lstat,
        ErrorKind::PermissionDenied,
    );

    let result = scan_app(
        &gate(&fake, &ALL_ROOTS),
        Path::new(BAR),
        home(),
        AggressiveItems::Unticked,
    );

    assert!(
        matches!(result, Err(Error::Fs(fs::Error::Io { op: Op::Lstat, .. }))),
        "{result:?}"
    );
}

/// The installed suite, with the folder `unsizable` inside a planned item failing to list.
fn with_unsizable_folder(unsizable: &str) -> FakeBackend {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    quiet(&fake);
    fake.add_file(format!("{unsizable}/x"), "x");
    fake.fail_on(unsizable, Op::ReadDir, ErrorKind::Other);
    fake
}

#[test]
fn an_item_that_cannot_be_sized_fails_the_scan() {
    let fails = |result: Result<Scanned, Error>| {
        matches!(
            result,
            Err(Error::Fs(fs::Error::Io {
                op: Op::ReadDir,
                ..
            }))
        )
    };
    let app_scan = |fake: &FakeBackend| {
        scan_app(
            &gate(fake, &ALL_ROOTS),
            Path::new(BAR),
            home(),
            AggressiveItems::Unticked,
        )
    };

    let bundle = with_unsizable_folder(&format!("{BAR}/Contents/Resources"));
    let leftover = with_unsizable_folder(&lib("Caches/com.foo.Bar/data"));
    let orphan = with_unsizable_folder(&lib("Caches/com.gone.App/data"));

    assert!(fails(app_scan(&bundle)), "the app's own bundle");
    assert!(fails(app_scan(&leftover)), "a leftover of the app");
    assert!(
        fails(scan_orphans(
            &gate(&orphan, &ALL_ROOTS),
            home(),
            AggressiveItems::Unticked
        )),
        "an orphan"
    );
}
