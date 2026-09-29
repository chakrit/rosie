//! Sandbox tier: scanning, running, the skip lines, first-run pulls, and exit codes.

mod cli_fixture;
mod pack_fixture;
mod tarball;

use std::io::ErrorKind;

use cli_fixture::{CARGO_RULE, CONFIG_FILE, CWD, HOME, NOW, ROSIE_URL, Sandbox, Script, at};
use pack_fixture::Canned;
use rosie::cli::ExitStatus;
use rosie::fs::Op;
use rosie::packs::MONTH;
use tarball::{TOP, Tarball};

// walk skips name the path and why

#[test]
fn each_walk_skip_names_its_path_and_reason() {
    let sandbox = Sandbox::new();
    sandbox.fake.add_dir(format!("{CWD}/Foo.app"));
    sandbox.fake.add_dir(format!("{CWD}/vol"));
    sandbox.fake.mount(format!("{CWD}/vol"), 2);
    sandbox.fake.add_dir(format!("{CWD}/cloud"));
    sandbox.fake.make_dataless(format!("{CWD}/cloud"));
    sandbox.fake.add_dir(format!("{CWD}/locked"));
    sandbox.fake.fail_on(
        format!("{CWD}/locked"),
        Op::ReadDir,
        ErrorKind::PermissionDenied,
    );

    let ran = sandbox.run(&["scan", "tree", "."]);

    ran.assert_status(ExitStatus::Success);
    for line in [
        format!("skipped {CWD}/Foo.app: bundle (use --enter-bundles to enter)"),
        format!("skipped {CWD}/vol: another volume (use --enter-mounts to cross)"),
        format!(
            "skipped {CWD}/cloud: cloud placeholder, not downloaded (use --enter-placeholders to open)"
        ),
        format!("skipped {CWD}/locked: permission denied"),
    ] {
        assert!(
            ran.stderr.lines().any(|shown| shown == line),
            "{line}\n{}",
            ran.stderr
        );
    }
    assert!(
        ran.stderr
            .contains("walk skipped 1 bundle, 1 mount, 1 placeholder, 1 denied"),
        "{}",
        ran.stderr
    );
}

#[test]
fn each_walk_flag_enters_what_it_names() {
    let cases = [
        ("--enter-bundles", "Foo.app"),
        ("--enter-mounts", "vol"),
        ("--enter-placeholders", "cloud"),
    ];
    for (flag, folder) in cases {
        let sandbox = Sandbox::new();
        let dir = format!("{CWD}/{folder}");
        sandbox.node_project(&format!("{dir}/app"));
        match folder {
            "vol" => sandbox.fake.mount(&dir, 2),
            "cloud" => sandbox.fake.make_dataless(&dir),
            _ => {}
        }
        let target = format!("{dir}/app/node_modules");

        let without = sandbox.run(&["scan", "tree", "."]);
        let with = sandbox.run(&["scan", "tree", ".", flag]);

        assert!(
            !without.stdout.contains(&target),
            "{flag}: {}",
            without.stdout
        );
        assert!(with.stdout.contains(&target), "{flag}: {}", with.stdout);
    }
}

#[test]
fn a_target_a_process_runs_from_is_skipped_naming_the_process() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));
    sandbox.processes(&format!(
        "  77     1   501 {CWD}/app/node_modules/.bin/vite\n"
    ));

    let ran = sandbox.run(&["scan", "tree", "."]);

    ran.assert_status(ExitStatus::Success);
    let line = format!(
        "skipped {CWD}/app/node_modules: in use by process 77 ({CWD}/app/node_modules/.bin/vite)"
    );
    assert!(
        ran.stderr.lines().any(|shown| shown == line),
        "{}",
        ran.stderr
    );
    assert!(!ran.stdout.contains("[[delete]]"), "{}", ran.stdout);
}

// run skips name the path and why

fn delete_entry(path: &str, sudo: bool) -> String {
    let sudo = match sudo {
        true => "sudo = true\n",
        false => "",
    };
    format!(
        "\n[[delete]]\npath = \"{path}\"\ntype = \"folder\"\nsize = 4096\nrules = [\"rosie/x\"]\nstatus = \"ticked\"\n{sudo}"
    )
}

fn plan(entries: &[String]) -> String {
    format!("version = 1\n{}", entries.concat())
}

#[test]
fn a_stale_entry_is_skipped_naming_why() {
    let sandbox = Sandbox::new();
    let gone = format!("{CWD}/gone");

    let ran = sandbox.run_with(
        Script::piped().stdin(&plan(&[delete_entry(&gone, false)])),
        &["run", "-"],
    );

    ran.assert_status(ExitStatus::Success);
    let line = format!("skipped (stale) {gone} (4.1 KB): no longer exists");
    assert!(
        ran.stderr.lines().any(|shown| shown == line),
        "{}",
        ran.stderr
    );
}

#[test]
fn an_entry_a_process_runs_from_is_skipped_at_run_naming_the_process() {
    let sandbox = Sandbox::new();
    let busy = format!("{CWD}/busy");
    sandbox.fake.add_file(format!("{busy}/bin/tool"), "x");
    sandbox.processes(&format!("  88     1   501 {busy}/bin/tool\n"));

    let ran = sandbox.run_with(
        Script::piped().stdin(&plan(&[delete_entry(&busy, false)])),
        &["run", "-"],
    );

    ran.assert_status(ExitStatus::Success);
    let line =
        format!("skipped (running process) {busy} (4.1 KB): process 88 runs {busy}/bin/tool");
    assert!(
        ran.stderr.lines().any(|shown| shown == line),
        "{}",
        ran.stderr
    );
    assert!(sandbox.exists(&format!("{busy}/bin/tool")));
}

#[test]
fn sudo_items_are_skipped_naming_why_when_sudo_fails() {
    let sandbox = Sandbox::new();
    let owned = format!("{CWD}/root-owned");
    sandbox.fake.add_file(format!("{owned}/f"), "x");
    sandbox.fake.chown(&owned, 0);

    let ran = sandbox.run_with(
        Script::piped().stdin(&plan(&[delete_entry(&owned, true)])),
        &["run", "-"],
    );

    ran.assert_status(ExitStatus::Success);
    let prefix = format!("skipped (sudo refused) {owned} (4.1 KB): ");
    assert!(
        ran.stderr.lines().any(|shown| shown.starts_with(&prefix)),
        "{}",
        ran.stderr
    );
    assert!(sandbox.exists(&format!("{owned}/f")));
}

// exit status

#[test]
fn a_failed_item_makes_rosie_exit_non_zero() {
    let sandbox = Sandbox::new();
    let item = format!("{CWD}/stuck");
    sandbox.fake.add_file(format!("{item}/f"), "x");
    sandbox.fake.fail_on(
        format!("{item}/f"),
        Op::RemoveFile,
        ErrorKind::PermissionDenied,
    );

    let ran = sandbox.run_with(
        Script::piped().stdin(&plan(&[delete_entry(&item, false)])),
        &["run", "-"],
    );

    ran.assert_status(ExitStatus::Failed);
    assert!(
        ran.stderr.contains(&format!("failed {item}")),
        "{}",
        ran.stderr
    );
    assert!(ran.stderr.contains("failed: 1 item"), "{}", ran.stderr);
}

#[test]
fn a_plan_of_another_version_is_an_error() {
    let sandbox = Sandbox::new();

    let ran = sandbox.run_with(Script::piped().stdin("version = 2\n"), &["run", "-"]);

    ran.assert_status(ExitStatus::Failed);
    assert!(sandbox.removals().is_empty());
}

#[test]
fn usage_errors_exit_with_the_usage_status() {
    let sandbox = Sandbox::new();

    for args in [
        &["scan"][..],
        &["scan", "tree", "--verbose"],
        &["scan", "app", "X.app", "--only", "node-modules"],
        &["scan", "app", "X.app", "--enter-bundles"],
        &["clean", "orphans", "--enter-placeholders"],
        &["clean", "tree", "--sh"],
        &["frobnicate"],
    ] {
        let ran = sandbox.run(args);
        ran.assert_status(ExitStatus::Usage);
    }
}

#[test]
fn help_and_version_print_on_stdout_and_succeed() {
    let sandbox = Sandbox::new();

    let help = sandbox.run(&["--help"]);
    let version = sandbox.run(&["--version"]);

    help.assert_status(ExitStatus::Success);
    version.assert_status(ExitStatus::Success);
    assert!(help.stdout.contains("clean"), "{}", help.stdout);
    assert!(!help.stdout.contains("__elevated"), "{}", help.stdout);
    assert_eq!(
        version.stdout.trim(),
        format!("rosie {}", env!("CARGO_PKG_VERSION"))
    );
}

// scan output and flags

#[test]
fn scan_prints_the_plan_on_stdout_and_the_stats_on_stderr() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));

    let ran = sandbox.run(&["scan", "tree", "."]);

    ran.assert_status(ExitStatus::Success);
    assert!(ran.stdout.starts_with("# rosie plan."), "{}", ran.stdout);
    assert!(
        ran.stdout
            .contains(&format!("path = \"{CWD}/app/node_modules\""))
    );
    assert!(ran.stderr.contains("ticked: 1 item"), "{}", ran.stderr);
    assert!(!ran.stdout.contains("ticked: 1 item"));
    assert!(sandbox.removals().is_empty());
}

#[test]
fn plan_is_an_alias_of_scan() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));

    let scanned = sandbox.run(&["scan", "tree"]);
    let planned = sandbox.run(&["plan", "tree"]);

    planned.assert_status(ExitStatus::Success);
    assert_eq!(planned.stdout, scanned.stdout);
}

#[test]
fn sh_prints_the_plan_as_a_shell_script() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));

    let ran = sandbox.run(&["scan", "tree", ".", "--sh"]);

    ran.assert_status(ExitStatus::Success);
    assert!(ran.stdout.starts_with("#!/bin/bash"), "{}", ran.stdout);
    assert!(ran.stdout.contains(&format!("{CWD}/app/node_modules")));
}

#[test]
fn only_limits_the_scan_to_the_named_rules() {
    let sandbox = Sandbox::new();
    sandbox.add_rules("cargo.toml", CARGO_RULE);
    sandbox.node_project(&format!("{CWD}/app"));
    sandbox.fake.add_file(format!("{CWD}/app/Cargo.toml"), "");
    sandbox
        .fake
        .add_file(format!("{CWD}/app/target/debug/app"), "x");

    let all = sandbox.run(&["scan", "tree", "."]);
    let only = sandbox.run(&["scan", "tree", ".", "--only", "cargo-target"]);
    let unknown = sandbox.run(&["scan", "tree", ".", "--only", "nope"]);

    assert!(all.stdout.contains("node_modules") && all.stdout.contains("target"));
    only.assert_status(ExitStatus::Success);
    assert!(
        only.stdout.contains(&format!("{CWD}/app/target")),
        "{}",
        only.stdout
    );
    assert!(!only.stdout.contains("node_modules"), "{}", only.stdout);
    unknown.assert_status(ExitStatus::Failed);
    assert!(unknown.stderr.contains("nope"), "{}", unknown.stderr);
}

#[test]
fn aggressive_ticks_the_aggressive_command_in_place_of_the_normal_one() {
    let sandbox = Sandbox::new();
    let docker = "[rules.docker]\nstrategy = \"tool\"\ncmd = \"docker system prune --force\"\ncmd_aggressive = \"docker system prune --all --force\"\n";
    sandbox.add_rules("docker.toml", docker);

    let normal = sandbox.run(&["scan", "caches"]);
    let aggressive = sandbox.run(&["scan", "caches", "--aggressive"]);

    assert!(
        normal.stderr.contains("ticked: 1 item"),
        "{}",
        normal.stderr
    );
    assert!(
        normal
            .stderr
            .contains("unticked (aggressive, not run): 1 item")
    );
    assert!(
        aggressive.stdout.contains("\"--all\""),
        "{}",
        aggressive.stdout
    );
    assert!(
        aggressive.stderr.contains("ticked: 1 item"),
        "{}",
        aggressive.stderr
    );
    assert!(
        !aggressive.stderr.contains("unticked"),
        "{}",
        aggressive.stderr
    );
}

// first run and pack age

fn default_source() -> Vec<u8> {
    Tarball::github()
        .dir(&format!("{TOP}/rules"))
        .file(
            &format!("{TOP}/rules/node.toml"),
            cli_fixture::NODE_RULE.as_bytes(),
        )
        .file(
            &format!("{TOP}/config.toml"),
            format!("roots = [\"{HOME}\"]\n").as_bytes(),
        )
        .gzip()
}

#[test]
fn a_first_run_pulls_the_default_pack_seeds_the_config_and_proceeds() {
    let sandbox = Sandbox::bare().with_net(Canned::serving(ROSIE_URL, default_source()));
    sandbox.node_project(&format!("{CWD}/app"));

    let ran = sandbox.run(&["scan", "tree", "."]);
    let first_requests = sandbox.net.requested();

    ran.assert_status(ExitStatus::Success);
    assert_eq!(
        first_requests,
        [ROSIE_URL],
        "one download seeds and installs"
    );
    assert!(
        ran.stderr.contains("pulled chakrit/rosie"),
        "{}",
        ran.stderr
    );
    assert!(
        ran.stderr.contains(&format!("created {CONFIG_FILE}")),
        "{}",
        ran.stderr
    );
    assert_eq!(sandbox.file(CONFIG_FILE), format!("roots = [\"{HOME}\"]\n"));
    assert!(
        ran.stdout.contains(&format!("{CWD}/app/node_modules")),
        "{}",
        ran.stdout
    );

    let again = sandbox.run(&["scan", "tree", "."]);
    again.assert_status(ExitStatus::Success);
    assert!(!again.stderr.contains("pulled"), "{}", again.stderr);
    assert_eq!(
        sandbox.net.requested(),
        first_requests,
        "the second run downloads nothing"
    );
}

#[test]
fn an_offline_first_run_stops_with_the_reason() {
    let sandbox = Sandbox::bare();
    sandbox.node_project(&format!("{CWD}/app"));

    let ran = sandbox.run(&["scan", "tree", "."]);

    ran.assert_status(ExitStatus::Failed);
    assert!(ran.stderr.contains("cannot download"), "{}", ran.stderr);
    assert!(ran.stderr.contains("unreachable"), "{}", ran.stderr);
    assert!(ran.stdout.is_empty());
}

#[test]
fn no_pack_present_makes_the_next_run_auto_pull() {
    let sandbox = Sandbox::bare().with_net(Canned::serving(ROSIE_URL, default_source()));
    sandbox.write_config(&format!("roots = [\"{HOME}\"]\n"));
    sandbox.node_project(&format!("{CWD}/app"));

    let ran = sandbox.run(&["scan", "tree", "."]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(
        sandbox.net.requested(),
        [ROSIE_URL],
        "one download pulls the pack"
    );
    assert!(
        ran.stderr.contains("pulled chakrit/rosie"),
        "{}",
        ran.stderr
    );
    assert!(
        ran.stdout.contains(&format!("{CWD}/app/node_modules")),
        "{}",
        ran.stdout
    );
}

#[test]
fn no_pack_present_while_offline_stops_with_the_reason() {
    let sandbox = Sandbox::bare();
    sandbox.write_config(&format!("roots = [\"{HOME}\"]\n"));
    sandbox.node_project(&format!("{CWD}/app"));

    let ran = sandbox.run(&["scan", "tree", "."]);

    ran.assert_status(ExitStatus::Failed);
    assert!(ran.stderr.contains("cannot download"), "{}", ran.stderr);
    assert!(ran.stderr.contains("unreachable"), "{}", ran.stderr);
    assert!(ran.stdout.is_empty());
}

#[test]
fn rules_six_months_old_draw_the_age_warning() {
    let fresh = Sandbox::new().at_time(at(NOW + 5 * MONTH.as_secs()));
    let old = Sandbox::new().at_time(at(NOW + 7 * MONTH.as_secs()));

    let fresh = fresh.run(&["scan", "tree", "."]);
    let old = old.run(&["scan", "tree", "."]);

    assert!(!fresh.stderr.contains("months old"), "{}", fresh.stderr);
    assert!(
        old.stderr
            .contains("Your rules are 7 months old, consider rosie rules pull to update"),
        "{}",
        old.stderr
    );
}
