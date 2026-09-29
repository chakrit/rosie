use super::*;

#[test]
fn symlinks_in_a_location_never_match() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_dir("/Users/me/elsewhere");
    fake.add_symlink(lib("Caches/com.foo.Bar"), "/Users/me/elsewhere");

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert_eq!(deletes(&scanned), [(BAR.to_owned(), Status::Ticked)]);
}

#[test]
fn refuses_an_app_argument_through_a_symlink() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_symlink("/Users/me/Bar.app", BAR);

    let result = scan_app(
        &gate(&fake, &ALL_ROOTS),
        Path::new("/Users/me/Bar.app"),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    );

    assert!(
        matches!(result, Err(Error::Fs(fs::Error::Symlink { .. }))),
        "{result:?}"
    );
}

/// Bar.app whose `Contents/Info.plist` is a symlink to a plist `plutil` could read.
fn bar_app_with_a_linked_plist(fake: &FakeBackend) {
    let plist = info_plist(Path::new(BAR));
    fake.add_sized_file(format!("{BAR}/Contents/MacOS/app"), 1 << 20);
    fake.add_file("/Users/me/real.plist", "<plist/>");
    fake.add_symlink(&plist, "/Users/me/real.plist");
    fake.respond(
        extract_argv(&plist, "CFBundleIdentifier"),
        succeeded("com.foo.Bar\n"),
    );
    fake.respond(extract_argv(&plist, "CFBundleName"), succeeded("Bar\n"));
    quiet(fake);
}

#[test]
fn a_symlinked_info_plist_is_refused_by_app_and_orphan_scans() {
    let fake = FakeBackend::new();
    bar_app_with_a_linked_plist(&fake);

    let app = scan_app(
        &gate(&fake, &ALL_ROOTS),
        Path::new(BAR),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    );
    let orphans = scan_orphans(
        &gate(&fake, &ALL_ROOTS),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    );

    assert!(
        matches!(app, Err(Error::Fs(fs::Error::Symlink { .. }))),
        "{app:?}"
    );
    assert!(
        matches!(orphans, Err(Error::UnreadableApp { ref source, .. }) if matches!(**source, Error::Fs(fs::Error::Symlink { .. }))),
        "{orphans:?}"
    );
}

// fixed folders: a symlink anywhere in their path fails the scan

fn is_symlink_refusal(result: &Result<Scan, Error>) -> bool {
    matches!(result, Err(Error::Fs(fs::Error::Symlink { .. })))
}

fn app_scan(fake: &FakeBackend, app: &str) -> Result<Scan, Error> {
    scan_app(
        &gate(fake, &ALL_ROOTS),
        Path::new(app),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    )
}

fn orphan_scan(fake: &FakeBackend) -> Result<Scan, Error> {
    scan_orphans(
        &gate(fake, &ALL_ROOTS),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    )
}

#[test]
fn a_symlinked_leftover_location_fails_app_and_orphan_scans() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    quiet(&fake);
    fake.add_dir("/Users/me/RealCaches/com.foo.Bar");
    fake.add_dir("/Users/me/RealCaches/com.gone.App");
    fake.add_symlink(lib("Caches"), "/Users/me/RealCaches");

    let app = app_scan(&fake, BAR);
    let orphans = orphan_scan(&fake);

    assert!(is_symlink_refusal(&app), "{app:?}");
    assert!(is_symlink_refusal(&orphans), "{orphans:?}");
}

#[test]
fn a_leftover_location_under_a_symlinked_library_fails_app_and_orphan_scans() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    quiet(&fake);
    fake.add_dir("/Users/me/RealLibrary/Caches/com.foo.Bar");
    fake.add_file("/Users/me/RealLibrary/Preferences/com.gone.App.plist", "x");
    fake.add_symlink(format!("{HOME}/Library"), "/Users/me/RealLibrary");

    let app = app_scan(&fake, BAR);
    let orphans = orphan_scan(&fake);

    assert!(is_symlink_refusal(&app), "{app:?}");
    assert!(is_symlink_refusal(&orphans), "{orphans:?}");
}

#[test]
fn a_symlinked_app_folder_fails_every_installed_app_listing() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    install_app(
        &fake,
        "/Users/me/RealApps/Qux.app",
        "com.qux.Qux",
        Some("Qux"),
    );
    fake.add_symlink(format!("{HOME}/Applications"), "/Users/me/RealApps");
    fake.add_dir(lib("Caches/com.qux.Qux"));

    let app = app_scan(&fake, BAR);
    let orphans = orphan_scan(&fake);
    let picker = picker_apps(&gate(&fake, &ALL_ROOTS), &home());

    assert!(is_symlink_refusal(&app), "{app:?}");
    assert!(is_symlink_refusal(&orphans), "{orphans:?}");
    assert!(
        matches!(picker, Err(Error::Fs(fs::Error::Symlink { .. }))),
        "{picker:?}"
    );
}

/// nPlayer.app, wrapped as an iPhone app, whose `Wrapper/nPlayer.app/Info.plist` is a
/// symlink to a plist `plutil` could read.
fn wrapped_app_with_a_linked_plist(fake: &FakeBackend) {
    let wrapped = format!("{NPLAYER}/Wrapper/nPlayer.app");
    let plist = PathBuf::from(format!("{wrapped}/Info.plist"));
    fake.add_sized_file(format!("{wrapped}/nPlayer"), 1 << 20);
    fake.add_file("/Users/me/real.plist", "<plist/>");
    fake.add_symlink(&plist, "/Users/me/real.plist");
    fake.respond(
        extract_argv(&plist, "CFBundleIdentifier"),
        succeeded("com.newin.nplayer\n"),
    );
    fake.respond(extract_argv(&plist, "CFBundleName"), succeeded("nPlayer\n"));
    quiet(fake);
}

#[test]
fn a_symlinked_wrapped_info_plist_is_refused_by_app_and_orphan_scans() {
    let fake = FakeBackend::new();
    wrapped_app_with_a_linked_plist(&fake);

    let app = app_scan(&fake, NPLAYER);
    let orphans = orphan_scan(&fake);

    assert!(is_symlink_refusal(&app), "{app:?}");
    assert!(
        matches!(orphans, Err(Error::UnreadableApp { ref source, .. }) if matches!(**source, Error::Fs(fs::Error::Symlink { .. }))),
        "{orphans:?}"
    );
}

#[test]
fn a_symlinked_wrapper_folder_is_refused() {
    let fake = FakeBackend::new();
    install_wrapped_app(&fake, "/Users/me/Real.app", "com.newin.nplayer", "nPlayer");
    fake.add_dir(format!("{NPLAYER}/Contents"));
    fake.add_symlink(format!("{NPLAYER}/Wrapper"), "/Users/me/Real.app/Wrapper");
    quiet(&fake);

    let app = app_scan(&fake, NPLAYER);

    assert!(is_symlink_refusal(&app), "{app:?}");
}

#[test]
fn a_symlinked_bundle_folder_for_reports_is_refused() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_dir("/Users/me/Items/BarHelper.app");
    fake.add_symlink(
        format!("{BAR}/Contents/Library/LoginItems"),
        "/Users/me/Items",
    );

    let app = app_scan(&fake, BAR);

    assert!(is_symlink_refusal(&app), "{app:?}");
}
