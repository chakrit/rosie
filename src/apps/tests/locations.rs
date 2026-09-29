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
    assert_eq!(scanned.skip_counts().total(), 0);
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
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
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
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
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
    let fails = |result: Result<Scan, Error>| {
        matches!(
            result,
            Err(Error::Sizing(crate::scan::Problem::Path(fs::Error::Io {
                op: Op::ReadDir,
                ..
            })))
        )
    };
    let app_scan = |fake: &FakeBackend| {
        scan_app(
            &gate(fake, &ALL_ROOTS),
            Path::new(BAR),
            &home(),
            AggressiveItems::Unticked,
            Mounts::Skip,
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
            &home(),
            AggressiveItems::Unticked,
            Mounts::Skip
        )),
        "an orphan"
    );
}

// volumes and placeholders

/// Sizing never crosses into another volume: a matched leftover that is itself a mount
/// point is skipped rather than planned, and a mount inside a planned leftover is left
/// out of its size.
#[test]
fn no_leftover_is_planned_or_sized_across_a_volume() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_sized_file(lib("Caches/com.foo.Bar/big"), 1 << 30);
    fake.mount(lib("Caches/com.foo.Bar"), 7);
    fake.add_sized_file(lib("Application Support/Bar/disk/big"), 1 << 30);
    fake.mount(lib("Application Support/Bar/disk"), 8);

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert_eq!(
        deletes(&scanned),
        [
            (BAR.to_owned(), Status::Ticked),
            (lib("Application Support/Bar"), Status::Unticked),
        ]
    );
    let support = entry(&scanned, &lib("Application Support/Bar"));
    assert!(support.size < Size::bytes(1 << 30), "{:?}", support.size);
    assert_eq!(scanned.skip_counts().mounts, 2);
}

/// A matched dataless placeholder is never planned, so its cloud copy is never at risk.
#[test]
fn a_matched_placeholder_is_skipped_not_planned() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_sized_file(lib("Containers/com.foo.Bar/data"), 4096);
    fake.make_dataless(lib("Containers/com.foo.Bar"));

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert_eq!(deletes(&scanned), [(BAR.to_owned(), Status::Ticked)]);
    assert_eq!(scanned.skip_counts().placeholders, 1);
}

/// A leftover location reached across another volume, counting from home for one under
/// home and from `/` otherwise, is a mount skip and is not listed unless
/// `enter_mounts` is on: the volume rule `caches` applies to a fixed rule path.
#[test]
fn a_leftover_location_behind_a_mount_is_searched_only_with_enter_mounts() {
    let scenario = || {
        let fake = FakeBackend::new();
        bar_app(&fake);
        fake.add_dir(lib("Caches/com.foo.Bar"));
        fake.add_file("/Library/LaunchDaemons/com.foo.Bar.helper.plist", "x");
        fake.mount(format!("{HOME}/Library"), 6);
        fake.mount("/Library", 5);
        fake
    };
    let scan = |fake: &FakeBackend, mounts: Mounts| {
        scan_app(
            &gate(fake, &ALL_ROOTS),
            Path::new(BAR),
            &home(),
            AggressiveItems::Unticked,
            mounts,
        )
        .expect("app scan")
    };
    let skips = |scanned: &Scan| -> Vec<(String, WalkSkip)> {
        let skipped = scanned.skipped.iter();
        skipped
            .map(|skip| (skip.path.display().to_string(), skip.reason))
            .collect()
    };

    let closed_fake = scenario();
    let closed = scan(&closed_fake, Mounts::Skip);
    let crossed = scan(&scenario(), Mounts::Enter);

    assert_eq!(deletes(&closed), [(BAR.to_owned(), Status::Ticked)]);
    assert_eq!(
        skips(&closed),
        [
            (
                "/Library/LaunchDaemons".to_owned(),
                WalkSkip::Closed(Needs::MOUNTS)
            ),
            (lib("Caches"), WalkSkip::Closed(Needs::MOUNTS)),
        ]
    );
    let calls = closed_fake.calls();
    assert!(!calls.contains(&crate::fs::fake::Call::ReadDir(lib("Caches").into())));
    assert!(!calls.contains(&crate::fs::fake::Call::ReadDir(
        "/Library/LaunchDaemons".into()
    )));
    assert_eq!(
        deletes(&crossed),
        [
            (BAR.to_owned(), Status::Ticked),
            (
                "/Library/LaunchDaemons/com.foo.Bar.helper.plist".to_owned(),
                Status::Ticked
            ),
            (lib("Caches/com.foo.Bar"), Status::Ticked),
        ]
    );
    assert!(skips(&crossed).is_empty());
}
