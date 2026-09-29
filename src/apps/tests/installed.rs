use super::*;

/// One way an installed app's bundle ID cannot be read, set up in the fake; the path
/// the scan must name.
#[derive(Debug, Clone, Copy)]
enum Unreadable {
    NoInfoPlist,
    MissingKey,
    CorruptPlist,
    InvalidId,
    UnlistableFolder,
}

impl Unreadable {
    const ALL: [Unreadable; 5] = [
        Unreadable::NoInfoPlist,
        Unreadable::MissingKey,
        Unreadable::CorruptPlist,
        Unreadable::InvalidId,
        Unreadable::UnlistableFolder,
    ];

    fn set_up(self, fake: &FakeBackend) -> &'static str {
        let respond_id = |bundle: &str, output| {
            let plist = info_plist(Path::new(bundle));
            fake.add_file(&plist, "<plist/>");
            fake.respond(extract_argv(&plist, "CFBundleIdentifier"), output);
        };
        match self {
            Unreadable::NoInfoPlist => {
                fake.add_dir("/Applications/NoPlist.app/Contents");
                "/Applications/NoPlist.app"
            }
            Unreadable::MissingKey => {
                let bundle = "/Applications/NoKey.app";
                let plist = info_plist(Path::new(bundle));
                respond_id(bundle, no_value(&plist, "CFBundleIdentifier"));
                bundle
            }
            Unreadable::CorruptPlist => {
                let bundle = "/Applications/Corrupt.app";
                respond_id(bundle, failed("Corrupt.app: Unexpected character\n"));
                bundle
            }
            Unreadable::InvalidId => {
                let bundle = "/Applications/Odd.app";
                respond_id(bundle, succeeded("com\n"));
                bundle
            }
            Unreadable::UnlistableFolder => {
                install_app(fake, "/Applications/Locked/In.app", "com.in.In", None);
                fake.fail_on(
                    "/Applications/Locked",
                    Op::ReadDir,
                    ErrorKind::PermissionDenied,
                );
                "/Applications/Locked"
            }
        }
    }
}

#[test]
fn an_installed_app_whose_id_cannot_be_read_fails_the_orphan_scan_naming_it() {
    // `MissingKey` is excluded: a plist with no bundle-ID key is a definite answer, not a
    // failed read, so it does not fail the scan. See
    // `an_installed_app_with_no_bundle_id_is_a_note_not_a_failure`.
    let failing = Unreadable::ALL
        .into_iter()
        .filter(|unreadable| !matches!(unreadable, Unreadable::MissingKey));
    for unreadable in failing {
        let fake = FakeBackend::new();
        installed_suite(&fake);
        let path = unreadable.set_up(&fake);
        fake.add_file(lib("Preferences/com.gone.App.plist"), "x");

        let result = scan_orphans(
            &gate(&fake, &ALL_ROOTS),
            &home(),
            AggressiveItems::Unticked,
            Mounts::Skip,
        );

        let named = match &result {
            Err(Error::UnreadableApp { bundle, .. }) => bundle == Path::new(path),
            Err(Error::UnlistableAppFolder { folder, .. }) => folder == Path::new(path),
            _ => false,
        };
        assert!(named, "{unreadable:?}: {result:?}");
        let message = result.expect_err("scan fails").to_string();
        assert!(message.contains(path), "{unreadable:?}: {message}");
    }
}

/// An installed app whose `Info.plist` has no bundle-ID key (a Steam "shortcut" app, on
/// a real Mac) has no bundle ID to claim or demote a leftover by, so it does not fail
/// the orphan scan. It is a report-only note there instead.
#[test]
fn an_installed_app_with_no_bundle_id_is_a_note_not_a_failure() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    let path = Unreadable::MissingKey.set_up(&fake);
    fake.add_file(lib("Caches/com.gone.App"), "x");

    let scanned = scan_orphans(
        &gate(&fake, &ALL_ROOTS),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    )
    .expect("scan");

    let subjects: Vec<&str> = scanned.plan.reports().iter().map(|r| r.subject()).collect();
    assert_eq!(
        subjects.iter().filter(|s| s.contains(path)).count(),
        1,
        "{subjects:?}"
    );
    assert_eq!(
        deletes(&scanned),
        [(lib("Caches/com.gone.App"), Status::Unticked)]
    );
}

#[test]
fn an_installed_app_whose_id_cannot_be_read_is_reported_by_app_scans() {
    // `MissingKey` is excluded: see
    // `another_installed_apps_missing_bundle_id_is_not_reported_by_app_scans`.
    let fake = FakeBackend::new();
    bar_app(&fake);
    let paths: Vec<&str> = Unreadable::ALL
        .iter()
        .filter(|unreadable| !matches!(unreadable, Unreadable::MissingKey))
        .map(|unreadable| unreadable.set_up(&fake))
        .collect();
    fake.add_file(lib("Preferences/com.foo.Bar.plist"), "x");

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    let subjects: Vec<&str> = scanned.plan.reports().iter().map(|r| r.subject()).collect();
    for path in paths {
        let named = subjects.iter().filter(|s| s.contains(path)).count();
        assert_eq!(named, 1, "{path} in {subjects:?}");
        let states = match path {
            "/Applications/Locked" => format!("app folder {path} cannot be listed"),
            _ => format!("bundle ID of the installed app {path} cannot be read"),
        };
        assert!(
            subjects.iter().any(|s| s.starts_with(&states)),
            "{states} in {subjects:?}"
        );
    }
    assert!(scanned.plan.reports().iter().all(|r| !r.steps().is_empty()));
    assert_eq!(
        entry(&scanned, &lib("Preferences/com.foo.Bar.plist")).status,
        Status::Ticked
    );
}

/// Another installed app with no bundle-ID key has no bundle ID to claim or demote a
/// leftover by, so an app scan, unlike the orphan scan, has no use for a note about it.
#[test]
fn another_installed_apps_missing_bundle_id_is_not_reported_by_app_scans() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    let path = Unreadable::MissingKey.set_up(&fake);

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    let subjects: Vec<&str> = scanned.plan.reports().iter().map(|r| r.subject()).collect();
    assert!(
        subjects.iter().all(|s| !s.contains(path)),
        "{path} unexpectedly noted in {subjects:?}"
    );
}

// wrapped iPhone and iPad apps

#[test]
fn a_wrapped_ios_app_is_installed_so_its_data_is_not_an_orphan() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    install_wrapped_app(&fake, NPLAYER, "com.newin.nplayer", "nPlayer");
    fake.add_dir(lib("Group Containers/group.com.newin.nplayer"));
    fake.add_dir(lib("Caches/com.gone.App"));

    let scanned = scanned_orphans(&fake, AggressiveItems::Ticked);

    assert_eq!(
        deletes(&scanned),
        [(lib("Caches/com.gone.App"), Status::Ticked)]
    );
}

#[test]
fn an_app_scan_reads_the_id_of_a_wrapped_ios_app() {
    let fake = FakeBackend::new();
    install_wrapped_app(&fake, NPLAYER, "com.newin.nplayer", "nPlayer");
    quiet(&fake);
    fake.add_dir(lib("Group Containers/group.com.newin.nplayer"));
    fake.add_dir(lib("Containers/com.newin.nplayer"));

    let scanned = scan_app(
        &gate(&fake, &ALL_ROOTS),
        Path::new(NPLAYER),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    )
    .expect("app scan");

    assert_eq!(entry(&scanned, NPLAYER).rules, ["app/com.newin.nplayer"]);
    assert_eq!(
        entry(&scanned, &lib("Containers/com.newin.nplayer")).status,
        Status::Ticked
    );
}

#[test]
fn a_wrapper_holding_several_apps_has_no_id() {
    let fake = FakeBackend::new();
    install_wrapped_app(&fake, NPLAYER, "com.newin.nplayer", "nPlayer");
    fake.add_file(
        format!("{NPLAYER}/Wrapper/Other.app/Info.plist"),
        "<plist/>",
    );
    quiet(&fake);

    let result = scan_app(
        &gate(&fake, &ALL_ROOTS),
        Path::new(NPLAYER),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    );

    assert!(
        matches!(result, Err(Error::NoInfoPlist { .. })),
        "{result:?}"
    );
}

// picker

#[test]
fn the_picker_lists_apps_in_both_application_folders() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    fake.add_symlink("/Applications/Linked.app", "/Users/me/Applications/Qux.app");
    fake.add_file("/Applications/.DS_Store", "x");

    let apps = picker_apps(&gate(&fake, &ALL_ROOTS), &home()).expect("listing");

    assert_eq!(
        apps,
        [
            PathBuf::from(BAR),
            PathBuf::from("/Applications/Suite/Baz.app"),
            PathBuf::from("/Users/me/Applications/Qux.app"),
        ]
    );
}

#[test]
fn an_install_folder_rosie_may_not_list_fails_the_orphan_scan_and_the_picker() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    fake.add_file(lib("Preferences/com.foo.Bar.plist"), "x");
    fake.fail_on("/Applications", Op::ReadDir, ErrorKind::PermissionDenied);

    let orphans = scan_orphans(
        &gate(&fake, &ALL_ROOTS),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    );
    let picker = picker_apps(&gate(&fake, &ALL_ROOTS), &home());

    assert!(
        matches!(orphans, Err(Error::UnlistableAppFolder { ref folder, .. }) if folder == Path::new("/Applications")),
        "{orphans:?}"
    );
    assert!(matches!(picker, Err(Error::Fs(_))), "{picker:?}");
}

/// One level down means only the `.app` folders directly in a plain subfolder such as
/// `/Applications/Utilities`: a plain folder or file there is not an app, and an app
/// nested deeper is not searched for. A plain subfolder is any folder not named
/// `<name>.app`, whatever its extension.
#[test]
fn only_app_folders_directly_in_a_plain_subfolder_are_installed_apps() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    fake.add_file("/Applications/Suite/Docs/Guide.pdf", "x");
    fake.add_file("/Applications/Suite/readme.txt", "x");
    install_app(
        &fake,
        "/Applications/Tools.localized/Tool.app",
        "com.tool.Tool",
        Some("Tool"),
    );
    install_app(
        &fake,
        "/Applications/Suite/Extras/Deep.app",
        "com.deep.Deep",
        Some("Deep"),
    );
    fake.add_dir(lib("Caches/com.gone.App"));

    let orphans = scan_orphans(
        &gate(&fake, &ALL_ROOTS),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    );
    let picker = picker_apps(&gate(&fake, &ALL_ROOTS), &home()).expect("listing");

    let orphans = orphans.expect("orphan scan");
    assert_eq!(
        deletes(&orphans),
        [(lib("Caches/com.gone.App"), Status::Unticked)]
    );
    assert_eq!(
        picker,
        [
            PathBuf::from(BAR),
            PathBuf::from("/Applications/Suite/Baz.app"),
            PathBuf::from("/Applications/Tools.localized/Tool.app"),
            PathBuf::from("/Users/me/Applications/Qux.app"),
        ]
    );
}

fn wrapped_app_scan(fake: &FakeBackend) -> Result<Scanned, Error> {
    scan_app(
        &gate(fake, &ALL_ROOTS),
        Path::new(NPLAYER),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    )
}

#[test]
fn entries_other_than_apps_in_a_wrapper_do_not_count_as_wrapped_apps() {
    let fake = FakeBackend::new();
    install_wrapped_app(&fake, NPLAYER, "com.newin.nplayer", "nPlayer");
    fake.add_file(
        format!("{NPLAYER}/Wrapper/BundleMetadata.plist"),
        "<plist/>",
    );
    fake.add_dir(format!("{NPLAYER}/Wrapper/Extras"));
    quiet(&fake);

    let scanned = wrapped_app_scan(&fake).expect("app scan");

    assert_eq!(entry(&scanned, NPLAYER).rules, ["app/com.newin.nplayer"]);
}

#[test]
fn a_wrapped_app_without_an_info_plist_has_no_id() {
    let fake = FakeBackend::new();
    fake.add_sized_file(format!("{NPLAYER}/Wrapper/nPlayer.app/nPlayer"), 1 << 20);
    quiet(&fake);

    let result = wrapped_app_scan(&fake);

    assert!(
        matches!(result, Err(Error::NoInfoPlist { .. })),
        "{result:?}"
    );
}

#[test]
fn a_wrapper_rosie_may_not_list_fails_the_scan_with_the_denial() {
    let fake = FakeBackend::new();
    install_wrapped_app(&fake, NPLAYER, "com.newin.nplayer", "nPlayer");
    fake.fail_on(
        format!("{NPLAYER}/Wrapper"),
        Op::ReadDir,
        ErrorKind::PermissionDenied,
    );
    quiet(&fake);

    let result = wrapped_app_scan(&fake);

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

/// The picker's order does not depend on which app folder is listed first: with a home
/// that sorts before `/Applications`, its apps come first.
#[test]
fn the_picker_lists_apps_sorted_by_path_across_folders() {
    let fake = FakeBackend::new();
    install_app(&fake, BAR, "com.foo.Bar", Some("Bar"));
    install_app(
        &fake,
        "/Accounts/me/Applications/Qux.app",
        "com.qux.Qux",
        Some("Qux"),
    );
    let gate = gate(&fake, &["/Applications", "/Accounts/me/Applications"]);

    let apps = picker_apps(
        &gate,
        &Home::new(Path::new("/Accounts/me")).expect("absolute home"),
    )
    .expect("listing");

    assert_eq!(
        apps,
        [
            PathBuf::from("/Accounts/me/Applications/Qux.app"),
            PathBuf::from(BAR),
        ]
    );
}

#[test]
fn an_app_folder_entry_rosie_cannot_lstat_fails_the_orphan_scan() {
    for hidden in ["/Applications/Hidden.app", "/Applications/Suite/Hidden.app"] {
        let fake = FakeBackend::new();
        installed_suite(&fake);
        fake.add_dir(hidden);
        fake.fail_on(hidden, Op::Lstat, ErrorKind::PermissionDenied);

        let result = scan_orphans(
            &gate(&fake, &ALL_ROOTS),
            &home(),
            AggressiveItems::Unticked,
            Mounts::Skip,
        );

        assert!(
            matches!(result, Err(Error::Fs(fs::Error::Io { op: Op::Lstat, .. }))),
            "{hidden}: {result:?}"
        );
    }
}
