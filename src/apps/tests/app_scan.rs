use super::*;

// app: matching

#[test]
fn lists_the_bundle_itself_ticked_under_the_apps_rule() {
    let fake = FakeBackend::new();
    bar_app(&fake);

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    let bundle = entry(&scanned, BAR);
    assert_eq!(bundle.status, Status::Ticked);
    assert_eq!(bundle.rules, ["app/com.foo.Bar"]);
    assert!(bundle.size.get() >= 1 << 20, "{}", bundle.size);
}

#[test]
fn bundle_id_matches_are_dot_bounded_and_ticked() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_file(lib("Preferences/com.foo.Bar.plist"), "x");
    fake.add_dir(lib("Caches/com.foo.Bar"));
    fake.add_dir(lib("Saved Application State/com.foo.Bar.savedState"));
    fake.add_file(lib("Preferences/com.foo.Barista.plist"), "x");
    fake.add_dir(lib("Caches/com.foo.Barista"));

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert_eq!(
        deletes(&scanned),
        [
            (BAR.to_owned(), Status::Ticked),
            (lib("Caches/com.foo.Bar"), Status::Ticked),
            (lib("Preferences/com.foo.Bar.plist"), Status::Ticked),
            (
                lib("Saved Application State/com.foo.Bar.savedState"),
                Status::Ticked
            ),
        ]
    );
}

#[test]
fn group_containers_match_the_team_prefixed_id() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_dir(lib("Group Containers/ABCDE12345.com.foo.Bar"));
    fake.add_dir(lib("Group Containers/ABCDE12345.com.foo.Barista"));
    fake.add_dir(lib("Caches/ABCDE12345.com.foo.Bar"));

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert_eq!(
        deletes(&scanned),
        [
            (BAR.to_owned(), Status::Ticked),
            (
                lib("Group Containers/ABCDE12345.com.foo.Bar"),
                Status::Ticked
            ),
        ]
    );
}

#[test]
fn name_only_matches_are_aggressive() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_dir(lib("Application Support/Bar"));
    fake.add_dir(lib("Logs/Bar"));
    fake.add_dir(lib("Application Support/Barista"));

    let unticked = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);
    let ticked = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Ticked);

    assert_eq!(
        deletes(&unticked)[1..],
        [
            (lib("Application Support/Bar"), Status::Unticked),
            (lib("Logs/Bar"), Status::Unticked),
        ]
    );
    assert_eq!(
        entry(&ticked, &lib("Logs/Bar")).status,
        Status::Ticked,
        "--aggressive ticks name-only matches"
    );
}

#[test]
fn an_app_without_a_bundle_name_has_no_name_only_matches() {
    let fake = FakeBackend::new();
    install_app(&fake, BAR, "com.foo.Bar", None);
    quiet(&fake);
    fake.add_dir(lib("Application Support/Bar"));

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert_eq!(deletes(&scanned), [(BAR.to_owned(), Status::Ticked)]);
}

/// A bundle can set `CFBundleName` to an empty string; `plutil -extract` then prints an
/// empty line rather than failing as it does for a missing key. An empty name must not
/// become a name-only match, since every dotfile name starts with `.` and would match.
#[test]
fn an_app_with_an_empty_bundle_name_has_no_name_only_matches() {
    let fake = FakeBackend::new();
    install_app(&fake, BAR, "com.foo.Bar", Some(""));
    quiet(&fake);
    fake.add_file(lib("Preferences/.DS_Store"), "");

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Ticked);

    assert_eq!(deletes(&scanned), [(BAR.to_owned(), Status::Ticked)]);
}

/// The app being scanned may itself have no bundle-ID key (a Steam "shortcut" app, on a
/// real Mac). It is still ticked, its `CFBundleName` still matches leftovers by name
/// (aggressively, like any name-only match), but it has no bundle ID to match a leftover
/// by, so a leftover that only looks like its bundle ID is left alone.
#[test]
fn an_app_with_no_bundle_id_plans_itself_and_its_name_matches_only() {
    let fake = FakeBackend::new();
    let plist = info_plist(Path::new(BAR));
    fake.add_file(&plist, "<plist/>");
    fake.add_sized_file(Path::new(BAR).join("Contents/MacOS/app"), 1 << 20);
    fake.respond(
        extract_argv(&plist, "CFBundleIdentifier"),
        no_value(&plist, "CFBundleIdentifier"),
    );
    fake.respond(extract_argv(&plist, "CFBundleName"), succeeded("Bar\n"));
    quiet(&fake);
    fake.add_dir(lib("Application Support/Bar"));
    fake.add_dir(lib("Application Support/com.foo.Bar"));

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert_eq!(
        deletes(&scanned),
        [
            (BAR.to_owned(), Status::Ticked),
            (lib("Application Support/Bar"), Status::Unticked),
        ]
    );
    assert_eq!(entry(&scanned, BAR).rules, ["app"]);
    assert!(scanned.plan.receipts().is_empty());
}

// app: launch jobs, receipts, reports

#[test]
fn launch_agents_boot_out_of_the_users_gui_domain_and_daemons_out_of_system() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_file(lib("LaunchAgents/com.foo.Bar.agent.plist"), "x");
    fake.add_file("/Library/LaunchDaemons/com.foo.Bar.helper.plist", "x");
    fake.chown("/Library/LaunchDaemons/com.foo.Bar.helper.plist", ROOT_UID);
    fake.add_file("/Library/PrivilegedHelperTools/com.foo.Bar.helper", "x");

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    let agent = entry(&scanned, &lib("LaunchAgents/com.foo.Bar.agent.plist"));
    let daemon = entry(&scanned, "/Library/LaunchDaemons/com.foo.Bar.helper.plist");
    let helper = entry(
        &scanned,
        "/Library/PrivilegedHelperTools/com.foo.Bar.helper",
    );
    assert_eq!(agent.bootouts[0].domain, LaunchDomain::Gui(USER_UID));
    assert_eq!(agent.bootouts[0].plist, agent.path);
    assert_eq!(daemon.bootouts[0].domain, LaunchDomain::System);
    assert!(helper.bootouts.is_empty());

    let booted = |run_as| -> Vec<PathBuf> {
        let runnable = scanned.plan.runnable_as(run_as);
        runnable.bootouts().map(|b| b.plist.clone()).collect()
    };
    assert_eq!(booted(RunAs::User), std::slice::from_ref(&agent.path));
    assert_eq!(booted(RunAs::Sudo), std::slice::from_ref(&daemon.path));
}

#[test]
fn items_the_user_does_not_own_are_marked_sudo() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.chown(BAR, ROOT_UID);
    fake.add_file("/Library/LaunchDaemons/com.foo.Bar.helper.plist", "x");
    fake.chown("/Library/LaunchDaemons/com.foo.Bar.helper.plist", ROOT_UID);
    fake.add_file(lib("Preferences/com.foo.Bar.plist"), "x");

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert_eq!(entry(&scanned, BAR).run_as, RunAs::Sudo);
    assert_eq!(
        entry(&scanned, "/Library/LaunchDaemons/com.foo.Bar.helper.plist").run_as,
        RunAs::Sudo
    );
    assert_eq!(
        entry(&scanned, &lib("Preferences/com.foo.Bar.plist")).run_as,
        RunAs::User
    );
}

#[test]
fn receipts_matching_the_bundle_id_are_forgotten() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    receipts(
        &fake,
        "com.apple.pkg.Core\ncom.foo.Bar\ncom.foo.Bar.pkg\ncom.foo.Barista.pkg\nABCDE12345.com.foo.Bar\n",
    );

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    let packages: Vec<&str> = scanned
        .plan
        .receipts()
        .iter()
        .map(|receipt| receipt.package.as_str())
        .collect();
    assert_eq!(packages, ["com.foo.Bar", "com.foo.Bar.pkg"]);
    assert!(
        scanned
            .plan
            .receipts()
            .iter()
            .all(|r| r.selection.is_ticked())
    );
}

#[test]
fn a_failing_receipt_listing_fails_the_scan() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.respond(pkgs_argv(), failed("pkgutil: broken\n"));

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
            Err(Error::Process(crate::process::Error::Failed { .. }))
        ),
        "{result:?}"
    );
}

#[test]
fn login_items_and_system_extensions_are_report_only() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_dir(format!("{BAR}/Contents/Library/LoginItems/BarHelper.app"));
    fake.add_dir(format!(
        "{BAR}/Contents/Library/SystemExtensions/com.foo.Bar.net.systemextension"
    ));

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    let subjects: Vec<&str> = scanned.plan.reports().iter().map(|r| r.subject()).collect();
    assert_eq!(
        subjects,
        [
            "login item BarHelper.app of Bar",
            "system extension com.foo.Bar.net.systemextension of Bar",
        ]
    );
    let before_deletion = |report: &crate::plan::Report| {
        report
            .steps()
            .iter()
            .any(|step| step.contains("Before Bar is deleted"))
    };
    let reports = scanned.plan.reports();
    assert!(
        !before_deletion(&reports[0]),
        "a login item can go after the app"
    );
    assert!(
        before_deletion(&reports[1]),
        "an extension goes before the app"
    );
    assert!(scanned.plan.reports().iter().all(|r| !r.steps().is_empty()));
    assert_eq!(
        scanned.plan.runnable_as(RunAs::User).deletes().len(),
        1,
        "reports never run"
    );
}

// app: refusals and roots

#[test]
fn refuses_the_plan_while_the_app_is_running() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    running(
        &fake,
        "    1     0     0 /sbin/launchd\n  812     1   501 /Applications/Bar.app/Contents/MacOS/Bar\n",
    );

    let result = scan_app(
        &gate(&fake, &ALL_ROOTS),
        Path::new(BAR),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    );

    assert!(
        matches!(result, Err(Error::Running { ref app, pid: 812 }) if app == Path::new(BAR)),
        "{result:?}"
    );
}

#[test]
fn a_process_from_a_sibling_app_does_not_block_the_plan() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    running(
        &fake,
        "  812     1   501 /Applications/Bar.app.old/Contents/MacOS/Bar\n",
    );

    let result = scan_app(
        &gate(&fake, &ALL_ROOTS),
        Path::new(BAR),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    );

    assert!(result.is_ok(), "{result:?}");
}

/// `docs/spec/safety.md#running-processes`: every plan, in every mode, refuses items
/// that any process is executing from.
#[test]
fn refuses_a_leftover_a_process_executes_from_and_plans_the_rest() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    let support = lib("Application Support/com.foo.Bar");
    fake.add_sized_file(format!("{support}/Updater"), 4096);
    fake.add_file(lib("Caches/com.foo.Bar/cache.db"), "x");
    running(
        &fake,
        &format!("    1     0     0 /sbin/launchd\n  900     1   501 {support}/Updater\n"),
    );

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Ticked);

    let planned = deletes(&scanned);
    assert!(
        planned.iter().all(|(path, _)| *path != support),
        "{planned:?}"
    );
    assert_eq!(entry(&scanned, BAR).status, Status::Ticked);
    assert_eq!(
        entry(&scanned, &lib("Caches/com.foo.Bar")).status,
        Status::Ticked
    );
    let [refused] = scanned.refused.as_slice() else {
        panic!("one refused item expected: {:?}", scanned.refused);
    };
    assert_eq!(refused.path, Path::new(&support));
    assert_eq!(refused.process.pid, 900);
    assert_eq!(refused.rules, ["app/com.foo.Bar"]);
}

#[test]
fn refuses_an_argument_that_is_not_an_app_bundle() {
    let fake = FakeBackend::new();
    fake.add_dir("/Users/me/code");
    quiet(&fake);

    let result = scan_app(
        &gate(&fake, &ALL_ROOTS),
        Path::new("/Users/me/code"),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    );

    assert!(matches!(result, Err(Error::NotAnApp { .. })), "{result:?}");
}

#[test]
fn items_outside_the_roots_are_blocked() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_file("/Library/LaunchDaemons/com.foo.Bar.helper.plist", "x");
    fake.add_file(lib("Preferences/com.foo.Bar.plist"), "x");

    let scanned = scanned_app(&fake, &["/Users/me/Library"], AggressiveItems::Unticked);

    assert_eq!(
        deletes(&scanned),
        [
            (BAR.to_owned(), Status::Blocked),
            (
                "/Library/LaunchDaemons/com.foo.Bar.helper.plist".to_owned(),
                Status::Blocked
            ),
            (lib("Preferences/com.foo.Bar.plist"), Status::Ticked),
        ]
    );
    assert!(
        scanned
            .plan
            .runnable_as(RunAs::User)
            .bootouts()
            .next()
            .is_none(),
        "a blocked plist is not booted out"
    );
}

#[test]
fn a_location_rosie_may_not_list_is_a_walk_skip() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_dir(lib("Containers/com.foo.Bar"));
    fake.fail_on(lib("Containers"), Op::ReadDir, ErrorKind::PermissionDenied);

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert_eq!(scanned.skip_counts().denied, 1);
    assert_eq!(deletes(&scanned), [(BAR.to_owned(), Status::Ticked)]);
}

// other copies of the app

#[test]
fn another_installed_copy_with_the_same_id_keeps_shared_leftovers_unticked() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    let beta = "/Applications/Bar Beta.app";
    install_app(&fake, beta, "com.foo.Bar", Some("Bar"));
    fake.add_file(lib("Preferences/com.foo.Bar.plist"), "x");
    receipts(&fake, "com.foo.Bar.pkg\n");

    let scanned = scan_app(
        &gate(&fake, &ALL_ROOTS),
        Path::new(beta),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    )
    .expect("app scan");

    assert_eq!(
        deletes(&scanned),
        [
            (beta.to_owned(), Status::Ticked),
            (lib("Preferences/com.foo.Bar.plist"), Status::Unticked),
        ]
    );
    assert!(!scanned.plan.receipts()[0].selection.is_ticked());
}

// app: launch folders, bundle contents, other installed apps

#[test]
fn folders_in_launch_folders_are_deleted_without_a_bootout() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_dir(lib("LaunchAgents/com.foo.Bar.agent"));
    fake.add_dir("/Library/LaunchDaemons/com.foo.Bar.helper");

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    let agent = entry(&scanned, &lib("LaunchAgents/com.foo.Bar.agent"));
    let daemon = entry(&scanned, "/Library/LaunchDaemons/com.foo.Bar.helper");
    assert!(agent.bootouts.is_empty(), "{:?}", agent.bootouts);
    assert!(daemon.bootouts.is_empty(), "{:?}", daemon.bootouts);
    assert!(
        scanned
            .plan
            .runnable_as(RunAs::User)
            .bootouts()
            .next()
            .is_none()
    );
}

#[test]
fn a_bundle_folder_rosie_may_not_list_for_reports_fails_the_scan() {
    for folder in ["LoginItems", "SystemExtensions"] {
        let fake = FakeBackend::new();
        bar_app(&fake);
        let denied = format!("{BAR}/Contents/Library/{folder}");
        fake.add_dir(format!("{denied}/Helper.app"));
        fake.fail_on(&denied, Op::ReadDir, ErrorKind::PermissionDenied);

        let result = scan_app(
            &gate(&fake, &ALL_ROOTS),
            Path::new(BAR),
            &home(),
            AggressiveItems::Unticked,
            Mounts::Skip,
        );

        assert!(matches!(result, Err(Error::Fs(_))), "{folder}: {result:?}");
    }
}

#[test]
fn matches_of_a_longer_installed_bundle_id_are_aggressive() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    install_app(
        &fake,
        "/Applications/Bar Canary.app",
        "com.foo.Bar.canary",
        Some("Bar Canary"),
    );
    fake.add_file(lib("Preferences/com.foo.Bar.plist"), "x");
    fake.add_file(lib("Preferences/com.foo.Bar.canary.plist"), "x");
    fake.add_dir(lib("Caches/com.foo.Bar.canary"));
    fake.add_dir(lib("Caches/com.foo.Bar.canaryish"));
    fake.add_dir(lib("Group Containers/ABCDE12345.com.foo.Bar.canary"));
    receipts(&fake, "com.foo.Bar.pkg\ncom.foo.Bar.canary.pkg\n");

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert_eq!(
        deletes(&scanned),
        [
            (BAR.to_owned(), Status::Ticked),
            (lib("Caches/com.foo.Bar.canary"), Status::Unticked),
            (lib("Caches/com.foo.Bar.canaryish"), Status::Ticked),
            (
                lib("Group Containers/ABCDE12345.com.foo.Bar.canary"),
                Status::Unticked
            ),
            (
                lib("Preferences/com.foo.Bar.canary.plist"),
                Status::Unticked
            ),
            (lib("Preferences/com.foo.Bar.plist"), Status::Ticked),
        ]
    );
    let receipts: Vec<(&str, bool)> = scanned
        .plan
        .receipts()
        .iter()
        .map(|r| (r.package.as_str(), r.selection.is_ticked()))
        .collect();
    assert_eq!(
        receipts,
        [("com.foo.Bar.pkg", true), ("com.foo.Bar.canary.pkg", false)]
    );
}

#[test]
fn a_system_extension_is_reported_when_the_bundle_has_no_login_items() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    fake.add_dir(format!(
        "{BAR}/Contents/Library/SystemExtensions/com.foo.Bar.net.systemextension"
    ));

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    let subjects: Vec<&str> = scanned.plan.reports().iter().map(|r| r.subject()).collect();
    assert_eq!(
        subjects,
        ["system extension com.foo.Bar.net.systemextension of Bar"]
    );
}

/// A leftover that is both a bundle-ID match and a name match is not a name-only match,
/// so it is ticked like any bundle-ID match; here the bundle's `CFBundleName` repeats its
/// ID.
#[test]
fn a_bundle_id_match_that_is_also_a_name_match_is_ticked() {
    let fake = FakeBackend::new();
    install_app(&fake, BAR, "com.foo.Bar", Some("com.foo.Bar"));
    quiet(&fake);
    fake.add_file(lib("Preferences/com.foo.Bar.plist"), "x");

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert_eq!(
        entry(&scanned, &lib("Preferences/com.foo.Bar.plist")).status,
        Status::Ticked
    );
}

/// Package IDs carry no team prefix, so a receipt is another installed app's only when
/// that app's ID claims it plainly. The IDs here are shaped so that reading the receipt
/// as a Group Containers name would hand it to the other app.
#[test]
fn a_receipt_is_taken_by_another_installed_app_only_through_its_plain_id() {
    let fake = FakeBackend::new();
    install_app(&fake, BAR, "ABCDE12345.ABCDE12345", Some("Bar"));
    install_app(
        &fake,
        "/Applications/Other.app",
        "ABCDE12345.ABCDE12345.x",
        Some("Other"),
    );
    quiet(&fake);
    receipts(&fake, "ABCDE12345.ABCDE12345.ABCDE12345.x\n");

    let scanned = scanned_app(&fake, &ALL_ROOTS, AggressiveItems::Unticked);

    assert!(scanned.plan.receipts()[0].selection.is_ticked());
}

#[test]
fn a_package_id_a_plan_cannot_hold_fails_the_scan() {
    let fake = FakeBackend::new();
    bar_app(&fake);
    receipts(&fake, "com.foo.Bar.\0pkg\n");

    let result = scan_app(
        &gate(&fake, &ALL_ROOTS),
        Path::new(BAR),
        &home(),
        AggressiveItems::Unticked,
        Mounts::Skip,
    );

    assert!(matches!(result, Err(Error::Plan(_))), "{result:?}");
}
