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

/// `docs/spec/safety.md#running-processes`: a daemon still running from an orphaned
/// helper keeps it out of the plan, even while its plist is planned for a bootout.
#[test]
fn refuses_an_orphan_a_process_executes_from() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    let helper = "/Library/PrivilegedHelperTools/com.old.helper";
    let plist = "/Library/LaunchDaemons/com.old.helper.plist";
    fake.add_sized_file(helper, 4096);
    fake.add_file(plist, "<plist/>");
    running(
        &fake,
        &format!("    1     0     0 /sbin/launchd\n  321     1     0 {helper}\n"),
    );

    let scanned = scanned_orphans(&fake, AggressiveItems::Ticked);

    assert_eq!(deletes(&scanned), [(plist.to_owned(), Status::Ticked)]);
    let [refused] = scanned.refused.as_slice() else {
        panic!("one refused item expected: {:?}", scanned.refused);
    };
    assert_eq!(refused.path, Path::new(helper));
    assert_eq!(refused.process.pid, 321);
    assert_eq!(refused.rules, ["orphans"]);
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
