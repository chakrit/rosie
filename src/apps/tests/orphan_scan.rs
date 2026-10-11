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
    fake.respond_running(&[(1, "/sbin/launchd"), (321, helper)]);

    let scanned = scanned_orphans(&fake, AggressiveItems::Ticked);

    assert_eq!(deletes(&scanned), [(plist.to_owned(), Status::Ticked)]);
    let [refused] = scanned.refused.as_slice() else {
        panic!("one refused item expected: {:?}", scanned.refused);
    };
    assert_eq!(refused.path, Path::new(helper));
    assert_eq!(refused.pid, 321);
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

// claimed IDs (`docs/spec/app.md#rosie-clean-orphans`): an ID something still claims is
// never an orphan, ticked or not

#[test]
fn an_id_sharing_an_installed_apps_vendor_prefix_is_not_an_orphan() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    fake.add_file("/Library/LaunchDaemons/com.foo.vmnetd.plist", "<plist/>");
    fake.add_dir(lib("Group Containers/ABCDE12345.com.foo.helper"));
    fake.add_dir(lib("Caches/com.food.App"));

    let scanned = scanned_orphans(&fake, AggressiveItems::Ticked);

    assert_eq!(
        deletes(&scanned),
        [(lib("Caches/com.food.App"), Status::Ticked)]
    );
}

#[test]
fn an_id_of_a_loaded_launch_job_is_not_an_orphan() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    loaded(
        &fake,
        "-\t0\torg.swift.swiftpm\n321\t0\tcom.gone.Agent.helper\n",
    );
    fake.add_file(lib("LaunchAgents/com.gone.Agent.helper.plist"), "<plist/>");
    fake.add_dir(lib("Caches/org.swift.swiftpm"));
    fake.add_dir(lib("Caches/com.gone.Agent"));

    let scanned = scanned_orphans(&fake, AggressiveItems::Ticked);

    assert_eq!(
        deletes(&scanned),
        [(lib("Caches/com.gone.Agent"), Status::Ticked)]
    );
}

#[test]
fn an_id_of_a_job_loaded_in_the_system_domain_is_not_an_orphan() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    loaded_system(
        &fake,
        "       0      - \tcom.docker.vmnetd\n     654   (pe) \tcom.vpn.helper",
    );
    fake.add_file("/Library/LaunchDaemons/com.docker.vmnetd.plist", "<plist/>");
    fake.add_file("/Library/PrivilegedHelperTools/com.vpn.helper", "x");
    fake.add_file(
        "/Library/LaunchDaemons/com.disabled.Daemon.plist",
        "<plist/>",
    );

    let scanned = scanned_orphans(&fake, AggressiveItems::Ticked);

    assert_eq!(
        deletes(&scanned),
        [(
            "/Library/LaunchDaemons/com.disabled.Daemon.plist".to_owned(),
            Status::Ticked
        )]
    );
}

#[test]
fn a_system_job_listing_rosie_cannot_read_fails_the_scan() {
    let malformed = FakeBackend::new();
    installed_suite(&malformed);
    loaded_system(&malformed, "0 - com.docker.vmnetd\ncom.half.line");
    let no_block = FakeBackend::new();
    installed_suite(&no_block);
    no_block.respond(
        system_jobs_argv(),
        succeeded("system = {\n\ttype = system\n}\n"),
    );

    let malformed = scan_orphans(
        &ctx(&malformed, &ALL_ROOTS),
        &paths(),
        AggressiveItems::Ticked,
        Mounts::Skip,
    );
    let no_block = scan_orphans(
        &ctx(&no_block, &ALL_ROOTS),
        &paths(),
        AggressiveItems::Ticked,
        Mounts::Skip,
    );

    assert!(
        matches!(&malformed, Err(Error::SystemJobs { line }) if line.contains("com.half.line")),
        "expected an unreadable services line, got {malformed:?}"
    );
    assert!(
        matches!(no_block, Err(Error::SystemJobsMissing)),
        "expected a missing services block, got {no_block:?}"
    );
}

#[test]
fn an_id_of_an_installed_homebrew_formula_is_not_an_orphan() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    fake.add_dir("/opt/homebrew/Cellar/redis/7.2.4");
    fake.add_dir("/usr/local/Cellar/mysql/8.0");
    fake.add_file(lib("LaunchAgents/homebrew.mxcl.redis.plist"), "<plist/>");
    fake.add_file(lib("LaunchAgents/homebrew.mxcl.mysql.plist"), "<plist/>");
    fake.add_file(lib("LaunchAgents/homebrew.mxcl.gone.plist"), "<plist/>");

    let scanned = scanned_orphans(&fake, AggressiveItems::Ticked);

    assert_eq!(
        deletes(&scanned),
        [(lib("LaunchAgents/homebrew.mxcl.gone.plist"), Status::Ticked)]
    );
}

#[test]
fn a_launch_job_listing_rosie_cannot_read_fails_the_scan() {
    let fake = FakeBackend::new();
    installed_suite(&fake);
    loaded(&fake, "-\t0\n");

    let result = scan_orphans(
        &ctx(&fake, &ALL_ROOTS),
        &paths(),
        AggressiveItems::Ticked,
        Mounts::Skip,
    );

    assert!(
        matches!(result, Err(Error::LaunchJobs { .. })),
        "expected a launch-job listing error, got {result:?}"
    );
}
