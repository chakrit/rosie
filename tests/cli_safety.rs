//! Sandbox tier, safety first: what the CLI refuses, and that it deletes only what the
//! confirmed plan holds, inside the roots.

mod cli_fixture;
mod pack_fixture;

use std::path::PathBuf;

use cli_fixture::{CWD, HOME, Sandbox, Script};
use rosie::cli::ExitStatus;
use rosie::fs::fake::Call;

// sudo rosie

#[test]
fn sudo_rosie_is_refused_before_anything_is_read() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));
    sandbox.running_as("0");

    let ran = sandbox.run_with(Script::terminal().answer("y"), &["clean", "tree", "."]);

    ran.assert_status(ExitStatus::Failed);
    let lines: Vec<&str> = ran.stderr.lines().collect();
    assert_eq!(lines.len(), 2, "{}", ran.stderr);
    let known = [
        "Jane! Stop this crazy thing!",
        "I swear on my mother's rechargeable batteries!",
        "rosie destroys things.",
    ];
    assert!(
        known.iter().any(|line| lines[0].starts_with(line)),
        "{}",
        ran.stderr
    );
    assert_eq!(lines[1], "run it again without sudo: rosie clean tree .");
    let read_anything = sandbox.fake.calls().into_iter().any(|call| {
        matches!(
            call,
            Call::ReadDir(_) | Call::ReadFile(_) | Call::WriteFile(_)
        )
    });
    assert!(!read_anything, "{:?}", sandbox.fake.calls());
    assert!(ran.asked.is_empty());
}

#[test]
fn the_elevated_entry_is_not_the_sudo_refusal_and_refuses_on_its_own_gates() {
    let sandbox = Sandbox::new();
    sandbox.running_as("0");

    let ran = sandbox.run(&["__elevated", "0123456789abcdef0123456789abcdef"]);

    ran.assert_status(ExitStatus::Failed);
    assert!(ran.stderr.contains("stdin is not a pipe"), "{}", ran.stderr);
    assert!(!ran.stderr.contains("without sudo"), "{}", ran.stderr);
}

// clean deletes only the confirmed plan, inside the roots

#[test]
fn clean_answered_yes_deletes_ticked_items_inside_the_roots_only() {
    let sandbox = Sandbox::with_roots(&["/Users/me/src/inside"]);
    sandbox.node_project(&format!("{CWD}/inside"));
    sandbox.node_project(&format!("{CWD}/outside"));

    let ran = sandbox.run_with(Script::terminal().answer("y"), &["clean", "tree", "."]);

    ran.assert_status(ExitStatus::Success);
    assert!(!sandbox.exists(format!("{CWD}/inside/node_modules").as_str()));
    assert!(sandbox.exists(format!("{CWD}/outside/node_modules").as_str()));
    assert!(sandbox.exists(format!("{CWD}/inside/package.json").as_str()));
    assert!(
        ran.stdout
            .contains(&format!("rosie roots add {CWD}/outside/node_modules")),
        "{}",
        ran.stdout
    );
    assert_eq!(ran.asked, ["Clean the ticked items? [y/N/e]"]);
}

#[test]
fn clean_answered_no_or_nothing_deletes_nothing() {
    for answer in ["n", "", "whatever"] {
        let sandbox = Sandbox::new();
        sandbox.node_project(&format!("{CWD}/app"));

        let ran = sandbox.run_with(Script::terminal().answer(answer), &["clean", "tree", "."]);

        ran.assert_status(ExitStatus::Success);
        assert!(sandbox.removals().is_empty(), "answer {answer:?}");
        assert!(ran.stderr.contains("nothing cleaned"), "{}", ran.stderr);
    }
}

#[test]
fn clean_edit_removes_unticked_entries_reshows_the_plan_and_asks_again() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/a"));
    sandbox.node_project(&format!("{CWD}/b"));
    let script = Script::terminal().answer("e").keep(&[1]).answer("y");

    let ran = sandbox.run_with(script, &["clean", "tree", "."]);

    ran.assert_status(ExitStatus::Success);
    assert!(sandbox.exists(format!("{CWD}/a/node_modules").as_str()));
    assert!(!sandbox.exists(format!("{CWD}/b/node_modules").as_str()));
    assert_eq!(ran.asked.len(), 3, "{:?}", ran.asked);
    assert!(ran.asked[1].contains(&format!("delete {CWD}/a/node_modules")));
    let shown = ran.stdout.matches("version = 1").count();
    assert_eq!(
        shown, 2,
        "the plan is shown, then shown trimmed:\n{}",
        ran.stdout
    );
    let trimmed = ran
        .stdout
        .rsplit("version = 1")
        .next()
        .expect("second plan");
    assert!(
        !trimmed.contains(&format!("{CWD}/a/node_modules")),
        "{trimmed}"
    );
}

#[test]
fn clean_without_a_terminal_is_a_usage_error_before_any_scan() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));

    let ran = sandbox.run_with(Script::piped(), &["clean", "tree", "."]);

    ran.assert_status(ExitStatus::Usage);
    assert!(ran.stderr.contains("needs a terminal"), "{}", ran.stderr);
    let walked = sandbox
        .fake
        .calls()
        .into_iter()
        .any(|call| call == Call::ReadDir(PathBuf::from(CWD)));
    assert!(!walked);
    assert!(sandbox.removals().is_empty());
}

#[test]
fn bare_rosie_cleans_the_current_folder_after_confirmation() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));
    sandbox.node_project(&format!("{HOME}/elsewhere"));

    let ran = sandbox.run_with(Script::terminal().answer("y"), &[]);

    ran.assert_status(ExitStatus::Success);
    assert!(!sandbox.exists(format!("{CWD}/app/node_modules").as_str()));
    assert!(sandbox.exists(format!("{HOME}/elsewhere/node_modules").as_str()));
}

// run

#[test]
fn run_refuses_a_hand_written_ticked_entry_outside_the_roots() {
    let sandbox = Sandbox::with_roots(&[CWD]);
    sandbox.fake.add_file("/Users/me/keep/data.txt", "x");
    let plan = "version = 1\n\n[[delete]]\npath = \"/Users/me/keep\"\ntype = \"folder\"\nsize = 4096\nrules = [\"rosie/x\"]\nstatus = \"ticked\"\n";

    let ran = sandbox.run_with(Script::piped().stdin(plan), &["run", "-"]);

    ran.assert_status(ExitStatus::Failed);
    assert!(sandbox.exists("/Users/me/keep/data.txt"));
    assert!(sandbox.removals().is_empty());
    assert!(
        ran.stderr.contains("outside the allowed roots"),
        "{}",
        ran.stderr
    );
}

#[test]
fn run_never_prompts_and_runs_only_ticked_entries() {
    let sandbox = Sandbox::new();
    sandbox.fake.add_file(format!("{CWD}/a/gone.txt"), "x");
    sandbox.fake.add_file(format!("{CWD}/b/kept.txt"), "x");
    let plan = format!(
        "version = 1\n\n\
         [[delete]]\npath = \"{CWD}/a\"\ntype = \"folder\"\nsize = 4096\nrules = [\"rosie/x\"]\nstatus = \"ticked\"\n\n\
         [[delete]]\npath = \"{CWD}/b\"\ntype = \"folder\"\nsize = 4096\nrules = [\"rosie/x\"]\nstatus = \"unticked\"\n"
    );
    sandbox.fake.add_file(format!("{HOME}/plan.toml"), plan);

    let ran = sandbox.run(&["run", "../plan.toml"]);

    ran.assert_status(ExitStatus::Success);
    assert!(ran.asked.is_empty());
    assert!(!sandbox.exists(format!("{CWD}/a").as_str()));
    assert!(sandbox.exists(format!("{CWD}/b/kept.txt").as_str()));
    assert!(
        ran.stderr.contains(&format!("deleted {CWD}/a")),
        "{}",
        ran.stderr
    );
}

// pickers refuse without a terminal

#[test]
fn every_picker_is_a_usage_error_without_a_terminal() {
    let cases: [&[&str]; 7] = [
        &["roots", "remove"],
        &["rules", "remove"],
        &["config", "get"],
        &["config", "set"],
        &["config", "unset"],
        &["clean", "app"],
        &["scan", "app"],
    ];
    for args in cases {
        let sandbox = Sandbox::new();
        let script = Script {
            terminals: rosie::cli::Terminals {
                stdin: false,
                stdout: true,
                stderr: true,
            },
            ..Script::terminal().pick(0)
        };

        let ran = sandbox.run_with(script, args);

        ran.assert_status(ExitStatus::Usage);
        assert!(ran.asked.is_empty(), "{args:?} showed {:?}", ran.asked);
        assert!(ran.stderr.contains("terminal"), "{args:?}: {}", ran.stderr);
    }
}

#[test]
fn a_picker_needs_stdout_to_be_a_terminal_too() {
    let sandbox = Sandbox::new();
    let script = Script {
        terminals: rosie::cli::Terminals {
            stdin: true,
            stdout: false,
            stderr: true,
        },
        ..Script::terminal().pick(0)
    };

    let ran = sandbox.run_with(script, &["roots", "remove"]);

    ran.assert_status(ExitStatus::Usage);
    assert!(ran.asked.is_empty());
}
