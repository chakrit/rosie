//! Fixture tier for targets a running process executes from: they stay out of the plan,
//! are sized apart, and what sizing meets inside them is not a walk skip
//! (`docs/spec/safety.md#running-processes`).

mod scan_fixture;

use std::io::ErrorKind;
use std::path::Path;

use rosie::fs::Op;
use rosie::plan::Size;
use rosie::scan::{Error, NoProgress};
use scan_fixture::*;

#[test]
fn targets_a_process_executes_from_are_refused() {
    let fake = node_home();
    add_rules(
        &fake,
        "fixed.toml",
        "[rules.z-pinned]\nstrategy = \"path\"\npaths = [\"~/code/app/node_modules\"]\n",
    );
    node_project(&fake, "/Users/me/code/app");
    node_project(&fake, "/Users/me/code/lib");
    running(
        &fake,
        "    1     0     0 /sbin/launchd\n\
            42     1   501 /Users/me/code/app/node_modules/.bin/server\n\
            43     1   501 node_modules\n",
    );

    let scan = tree(&fake, CODE);

    assert_eq!(paths(&scan), ["/Users/me/code/lib/node_modules"]);
    let [refused] = &scan.refused[..] else {
        panic!("one refusal expected: {:?}", scan.refused);
    };
    assert_eq!(refused.path, Path::new("/Users/me/code/app/node_modules"));
    assert_eq!(refused.rules, ["rosie/node-modules", "rosie/z-pinned"]);
    assert_eq!(refused.process.pid, 42);
    assert_eq!(refused.size, Size::bytes(4096));
}

#[test]
fn a_refused_target_is_sized_apart_from_the_plan() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_file("/Users/me/code/lib/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/shared.js", 10_000);
    fake.add_hardlink(
        "/Users/me/code/app/node_modules/shared.js",
        "/Users/me/code/app/node_modules/again.js",
    );
    fake.add_hardlink(
        "/Users/me/code/app/node_modules/shared.js",
        "/Users/me/code/lib/node_modules/shared.js",
    );
    fake.add_sized_file("/Users/me/code/app/node_modules/own.js", 300);
    running(
        &fake,
        "   42     1   501 /Users/me/code/app/node_modules/own.js\n",
    );
    let progress = Recording::default();

    let scan = scanner(&fake, Options::default())
        .tree(Path::new(CODE), &progress)
        .expect("scan runs");

    let [refused] = &scan.refused[..] else {
        panic!("one refusal expected: {:?}", scan.refused);
    };
    assert_eq!(refused.size, Size::bytes(10_300));
    assert_eq!(size_of(&scan, "/Users/me/code/lib/node_modules"), 10_000);
    assert_eq!(progress.bytes.into_inner(), 10_000);
    assert_eq!(progress.found.into_inner(), 1);
}

#[test]
fn a_mount_inside_a_refused_target_is_not_counted_as_a_scan_skip() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/ok.js", 100);
    fake.add_sized_file("/Users/me/code/app/node_modules/mnt/huge", 1_000_000);
    fake.mount("/Users/me/code/app/node_modules/mnt", 9);
    running(
        &fake,
        "   42     1   501 /Users/me/code/app/node_modules/ok.js\n",
    );

    let scan = tree(&fake, CODE);

    let [refused] = &scan.refused[..] else {
        panic!("one refusal expected: {:?}", scan.refused);
    };
    assert_eq!(refused.size, Size::bytes(100));
    assert_eq!(skipped(&scan), []);
    assert_eq!(scan.skip_counts().mounts, 0);
}

#[test]
fn a_denied_folder_inside_a_refused_target_is_not_counted_as_a_scan_skip() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/ok.js", 100);
    fake.add_sized_file("/Users/me/code/app/node_modules/locked/x.js", 900);
    fake.fail_on(
        "/Users/me/code/app/node_modules/locked",
        Op::ReadDir,
        ErrorKind::PermissionDenied,
    );
    running(
        &fake,
        "   42     1   501 /Users/me/code/app/node_modules/ok.js\n",
    );

    let scan = tree(&fake, CODE);

    let [refused] = &scan.refused[..] else {
        panic!("one refusal expected: {:?}", scan.refused);
    };
    assert_eq!(refused.size, Size::bytes(100));
    assert_eq!(skipped(&scan), []);
    assert_eq!(scan.skip_counts().denied, 0);
    assert!(scan.problems.is_empty(), "{:?}", problems(&scan));
}

#[test]
fn a_non_permission_error_inside_a_refused_target_is_a_problem_not_a_silent_loss() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/ok.js", 100);
    fake.add_sized_file("/Users/me/code/app/node_modules/broken/x.js", 900);
    fake.fail_on(
        "/Users/me/code/app/node_modules/broken",
        Op::ReadDir,
        ErrorKind::InvalidData,
    );
    running(
        &fake,
        "   42     1   501 /Users/me/code/app/node_modules/ok.js\n",
    );

    let scan = tree(&fake, CODE);

    let [refused] = &scan.refused[..] else {
        panic!("one refusal expected: {:?}", scan.refused);
    };
    assert_eq!(refused.size, Size::bytes(100));
    assert_eq!(skipped(&scan), []);
    let [problem] = problems(&scan).try_into().expect("one problem");
    assert!(
        problem.contains("/Users/me/code/app/node_modules/broken"),
        "{problem}"
    );
}

#[test]
fn a_denied_entry_inside_a_refused_target_is_not_counted_as_a_scan_skip() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/ok.js", 100);
    fake.add_sized_file("/Users/me/code/app/node_modules/secret.js", 900);
    fake.fail_on(
        "/Users/me/code/app/node_modules/secret.js",
        Op::Lstat,
        ErrorKind::PermissionDenied,
    );
    running(
        &fake,
        "   42     1   501 /Users/me/code/app/node_modules/ok.js\n",
    );

    let scan = tree(&fake, CODE);

    let [refused] = &scan.refused[..] else {
        panic!("one refusal expected: {:?}", scan.refused);
    };
    assert_eq!(refused.size, Size::bytes(100));
    assert_eq!(skipped(&scan), []);
    assert_eq!(scan.skip_counts().denied, 0);
    assert!(scan.problems.is_empty(), "{:?}", problems(&scan));
}

#[test]
fn a_placeholder_inside_a_refused_target_is_not_counted_as_a_scan_skip() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/ok.js", 100);
    fake.add_sized_file("/Users/me/code/app/node_modules/cloud.bin", 900);
    fake.make_dataless("/Users/me/code/app/node_modules/cloud.bin");
    running(
        &fake,
        "   42     1   501 /Users/me/code/app/node_modules/ok.js\n",
    );

    let scan = tree(&fake, CODE);

    let [refused] = &scan.refused[..] else {
        panic!("one refusal expected: {:?}", scan.refused);
    };
    assert_eq!(refused.size, Size::bytes(100));
    assert_eq!(skipped(&scan), []);
    assert_eq!(scan.skip_counts().placeholders, 0);
}

#[test]
fn a_failing_process_table_fails_the_scan() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/app");
    fake.respond(
        rosie::process::ProcessTable::argv(),
        rosie::fs::CommandOutput {
            exit: rosie::fs::Exit::Code(1),
            stdout: Vec::new(),
            stderr: b"ps: no".to_vec(),
        },
    );

    let result = scanner(&fake, Options::default()).tree(Path::new(CODE), &NoProgress);

    assert!(matches!(result, Err(Error::Process(_))), "{result:?}");
}
