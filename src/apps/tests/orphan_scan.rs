use super::*;

#[test]
fn orphans_are_leftovers_no_installed_app_accounts_for() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    fake.add_file(lib("Preferences/com.foo.Bar.plist"), "x");
    fake.add_dir(lib("Caches/com.baz.Baz"));
    fake.add_dir(lib("Containers/com.qux.Qux"));
    fake.add_file(lib("Preferences/org.sys.Sys.plist"), "x");
    fake.add_dir(lib("Group Containers/group.com.foo.Bar"));
    fake.add_file(lib("Preferences/com.gone.App.plist"), "x");
    fake.add_dir(lib("Caches/com.gone.App"));

    let scanned = scanned_orphans(&fake, AggressiveItems::Unticked);

    assert_eq!(
        deletes(&scanned),
        [
            (lib("Caches/com.gone.App"), Status::Unticked),
            (lib("Preferences/com.gone.App.plist"), Status::Unticked),
        ]
    );
    assert!(entry(&scanned, &lib("Caches/com.gone.App")).rules == ["orphans"]);
}

#[test]
fn orphans_exclude_apple_and_names_that_are_not_bundle_ids() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    fake.add_file(lib("Preferences/com.apple.Safari.plist"), "x");
    fake.add_dir(lib("Group Containers/ABCDE12345.com.apple.shared"));
    fake.add_dir(lib("Application Support/Slack"));
    fake.add_file(lib("Logs/install.log"), "x");

    let scanned = scanned_orphans(&fake, AggressiveItems::Unticked);

    assert_eq!(deletes(&scanned), []);
}

#[test]
fn orphan_launch_jobs_are_booted_out_and_aggressive_ticks_orphans() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    fake.add_file(lib("LaunchAgents/com.gone.App.agent.plist"), "x");
    fake.add_dir(lib("Group Containers/ABCDE12345.com.gone.App"));

    let scanned = scanned_orphans(&fake, AggressiveItems::Ticked);

    let agent = entry(&scanned, &lib("LaunchAgents/com.gone.App.agent.plist"));
    assert_eq!(agent.status, Status::Ticked);
    assert_eq!(agent.bootouts[0].domain, LaunchDomain::Gui(USER_UID));
    assert_eq!(
        entry(&scanned, &lib("Group Containers/ABCDE12345.com.gone.App")).status,
        Status::Ticked
    );
}

// entries that vanish while scanning

#[test]
fn an_entry_that_vanishes_after_listing_is_passed_over() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    fake.add_dir(lib("Caches/com.gone.App"));
    fake.add_dir(lib("Caches/com.gone.Temp"));
    fake.fail_on(lib("Caches/com.gone.Temp"), Op::Lstat, ErrorKind::NotFound);
    fake.add_dir("/Applications/Removed.app");
    fake.fail_on("/Applications/Removed.app", Op::Lstat, ErrorKind::NotFound);

    let scanned = scanned_orphans(&fake, AggressiveItems::Unticked);

    assert_eq!(
        deletes(&scanned),
        [(lib("Caches/com.gone.App"), Status::Unticked)]
    );
}

#[test]
fn a_case_variant_of_an_installed_id_is_not_an_orphan() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    fake.add_file(lib("Preferences/com.foo.bar.plist"), "x");
    fake.add_dir(lib("Caches/com.BAZ.Baz"));

    let scanned = scanned_orphans(&fake, AggressiveItems::Unticked);

    assert_eq!(deletes(&scanned), []);
}
