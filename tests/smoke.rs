//! Smoke tier: the real `rosie` binary on temp folders, safety first
//! (`docs/spec/testing.md#tiers`). Every test runs with `env_clear()` and a sandboxed
//! `HOME` and `PATH`; the pack folder is pre-seeded so nothing reaches the network, and
//! nothing here touches the real home or real `sudo`.

mod smoke_fixture;

use smoke_fixture::{SmokeHome, node_project, stderr, stdout};

// --version / --help: read the environment only when a command needs it

#[test]
fn version_and_help_do_not_require_home() {
    for flag in ["--version", "--help"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_rosie"))
            .arg(flag)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("run the rosie binary");

        assert!(output.status.success(), "{flag}: {:?}", output);
    }
}

// scan: prints a plan, deletes nothing

#[test]
fn scan_tree_prints_a_plan_and_deletes_nothing() {
    let home = SmokeHome::with_roots(&["project"]);
    node_project(&home.path("project/app"));

    let scanned = home.run(
        &["scan", "tree", &home.path("project").display().to_string()],
        None,
    );

    assert!(scanned.status.success(), "{}", stderr(&scanned));
    let plan = stdout(&scanned);
    assert!(plan.contains("version = 1"), "{plan}");
    assert!(plan.contains("status = \"ticked\""), "{plan}");
    assert!(home.path("project/app/node_modules").exists());
}

// scan | run: deletes exactly the planned targets

#[test]
fn scan_piped_to_run_deletes_exactly_the_planned_targets() {
    let home = SmokeHome::with_roots(&["project"]);
    let app = home.path("project/app");
    node_project(&app);
    // A sibling source file, right beside the matched `node_modules`.
    std::fs::write(app.join("index.js"), "x").expect("write a source file");
    // A file outside the matched folder that a symlink inside it points to.
    let target = home.path("project/kept-target.txt");
    std::fs::write(&target, "x").expect("write a symlink target");
    std::os::unix::fs::symlink(&target, app.join("node_modules/to-target"))
        .expect("create a symlink inside the matched folder");

    let scanned = home.run(
        &["scan", "tree", &home.path("project").display().to_string()],
        None,
    );
    assert!(scanned.status.success(), "{}", stderr(&scanned));
    let plan = stdout(&scanned);

    let ran = home.run(&["run", "-"], Some(&plan));

    assert!(ran.status.success(), "{}", stderr(&ran));
    assert!(!app.join("node_modules").exists(), "the matched folder");
    assert!(app.join("index.js").exists(), "the sibling source file");
    assert!(target.exists(), "the symlink's target");
}

// out-of-roots: blocked by scan, and refused by run even when ticked by hand

#[test]
fn an_out_of_roots_target_is_blocked_and_survives_a_hand_ticked_run() {
    let home = SmokeHome::with_roots(&["allowed"]);
    let outside = home.path("elsewhere/app");
    node_project(&outside);

    let scanned = home.run(
        &[
            "scan",
            "tree",
            &home.path("elsewhere").display().to_string(),
        ],
        None,
    );
    assert!(scanned.status.success(), "{}", stderr(&scanned));
    let plan = stdout(&scanned);
    assert!(plan.contains("status = \"blocked\""), "{plan}");
    assert!(plan.contains("rosie roots add"), "{plan}");

    // Ticking the entry by hand takes it past the plan's own status, to the roots gate.
    let ticked = plan.replace("status = \"blocked\"", "status = \"ticked\"");

    let ran = home.run(&["run", "-"], Some(&ticked));

    assert_eq!(ran.status.code(), Some(1), "{}", stderr(&ran));
    assert!(outside.join("node_modules").exists());
    assert!(
        stderr(&ran).contains("outside the allowed roots"),
        "{}",
        stderr(&ran)
    );
}

// __elevated: refuses when not root

#[test]
fn elevated_refuses_when_not_root() {
    let home = SmokeHome::with_roots(&["project"]);
    let nonce = "0123456789abcdef0123456789abcdef";
    let header = format!(
        "rosie-elevated 1\nnonce {nonce}\nhome {}\n\n",
        home.home().display()
    );

    let ran = home.run(&["__elevated", nonce], Some(&header));

    assert!(!ran.status.success());
    assert_eq!(ran.status.code(), Some(1));
    assert!(
        stderr(&ran).contains("refused: not running as root"),
        "{}",
        stderr(&ran)
    );
}

// --sh: valid bash

#[test]
fn sh_output_parses_with_bash_n() {
    let home = SmokeHome::with_roots(&["project"]);
    node_project(&home.path("project/app"));

    let scanned = home.run(
        &[
            "scan",
            "tree",
            &home.path("project").display().to_string(),
            "--sh",
        ],
        None,
    );

    assert!(scanned.status.success(), "{}", stderr(&scanned));
    let script = stdout(&scanned);
    let checked = std::process::Command::new("/bin/bash")
        .arg("-n")
        .arg("-c")
        .arg(&script)
        .env_clear()
        .output()
        .expect("run bash -n");
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
}

// clean without a terminal: usage error

#[test]
fn clean_without_a_terminal_is_a_usage_error() {
    let home = SmokeHome::with_roots(&["project"]);
    node_project(&home.path("project/app"));

    let ran = home.run(
        &["clean", "tree", &home.path("project").display().to_string()],
        None,
    );

    assert_eq!(ran.status.code(), Some(2), "{}", stderr(&ran));
    assert!(stderr(&ran).contains("terminal"), "{}", stderr(&ran));
    assert!(home.path("project/app/node_modules").exists());
}
