//! Sandbox tier: `rosie check <path>…`, one line per path saying what a plan would do
//! with it (`docs/spec/cli.md#check`).

mod cli_fixture;
mod pack_fixture;
mod tarball;

use std::io::ErrorKind;

use cli_fixture::{CWD, HOME, Ran, Sandbox};
use rosie::cli::ExitStatus;
use rosie::fs::{Argv, Op};

/// A rule that calls a Python project's `.venv` aggressive.
const VENV_RULE: &str = "[rules.python-venv]\nstrategy = \"marker\"\n\
    target_aggressive = \".venv\"\nmarker = [\"pyproject.toml\"]\n";

/// A second rule calling `node_modules` aggressive, beside the pack's normal one.
const AGGRESSIVE_NODE_RULE: &str = "[rules.deps]\nstrategy = \"marker\"\n\
    target_aggressive = \"node_modules\"\nmarker = [\"package.json\"]\n";

/// One printed line, split into its three columns.
#[derive(Debug, PartialEq, Eq)]
struct Line {
    status: String,
    path: String,
    reason: String,
}

/// The line `check` printed for the path shown as `shown`.
fn line_for(ran: &Ran, shown: &str) -> Line {
    let lines: Vec<Line> = ran.stdout.lines().map(columns).collect();
    lines
        .into_iter()
        .find(|line| line.path == shown)
        .unwrap_or_else(|| panic!("no line for {shown}:\n{}", ran.stdout))
}

/// Columns are separated by at least two spaces; no column holds two spaces in a row.
fn columns(text: &str) -> Line {
    let parts: Vec<&str> = text
        .split("  ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    let [status, path, reason] = parts.as_slice() else {
        panic!("expected three columns in {text:?}");
    };
    Line {
        status: (*status).to_owned(),
        path: (*path).to_owned(),
        reason: (*reason).to_owned(),
    }
}

fn line(status: &str, path: &str, reason: &str) -> Line {
    Line {
        status: status.to_owned(),
        path: path.to_owned(),
        reason: reason.to_owned(),
    }
}

fn check(sandbox: &Sandbox, args: &[&str]) -> Ran {
    let argv: Vec<&str> = std::iter::once("check")
        .chain(args.iter().copied())
        .collect();
    sandbox.run(&argv)
}

// plan words for would-be entries

#[test]
fn a_matched_folder_is_clean_naming_its_rule_and_tier() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));

    let ran = check(&sandbox, &[&format!("{CWD}/app/node_modules")]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(
        line_for(&ran, "~/src/app/node_modules"),
        line(
            "clean",
            "~/src/app/node_modules",
            "rosie/node-modules (normal)"
        )
    );
}

#[test]
fn a_path_inside_a_matched_folder_names_the_entry_holding_it() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));

    let ran = check(&sandbox, &["app/node_modules/left-pad"]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(
        line_for(&ran, "~/src/app/node_modules/left-pad"),
        line(
            "clean",
            "~/src/app/node_modules/left-pad",
            "inside ~/src/app/node_modules (rosie/node-modules)"
        )
    );
}

#[test]
fn an_item_not_owned_by_the_user_needs_sudo_at_and_inside_it() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));
    sandbox.fake.chown(format!("{CWD}/app/node_modules"), 0);

    let ran = check(&sandbox, &["app/node_modules", "app/node_modules/left-pad"]);

    assert_eq!(
        line_for(&ran, "~/src/app/node_modules").reason,
        "rosie/node-modules (normal); needs sudo"
    );
    assert_eq!(
        line_for(&ran, "~/src/app/node_modules/left-pad").reason,
        "inside ~/src/app/node_modules (rosie/node-modules); needs sudo"
    );
}

#[test]
fn an_aggressive_item_is_keep_unless_aggressive_is_given() {
    let sandbox = Sandbox::new();
    sandbox.add_rules("venv.toml", VENV_RULE);
    sandbox
        .fake
        .add_file(format!("{CWD}/old/pyproject.toml"), "");
    sandbox
        .fake
        .add_file(format!("{CWD}/old/.venv/bin/python"), "x");

    let kept = check(&sandbox, &["old/.venv"]);
    let cleaned = check(&sandbox, &["old/.venv", "--aggressive"]);

    kept.assert_status(ExitStatus::Failed);
    assert_eq!(
        line_for(&kept, "~/src/old/.venv"),
        line(
            "keep",
            "~/src/old/.venv",
            "rosie/python-venv (aggressive); --aggressive cleans it"
        )
    );
    cleaned.assert_status(ExitStatus::Success);
    assert_eq!(
        line_for(&cleaned, "~/src/old/.venv"),
        line("clean", "~/src/old/.venv", "rosie/python-venv (aggressive)")
    );
}

#[test]
fn an_item_outside_the_roots_names_the_roots_add_that_allows_it() {
    let sandbox = Sandbox::with_roots(&[CWD]);
    sandbox.node_project(&format!("{HOME}/other/app"));

    let ran = check(&sandbox, &[&format!("{HOME}/other/app/node_modules")]);

    ran.assert_status(ExitStatus::Failed);
    assert_eq!(
        line_for(&ran, "~/other/app/node_modules"),
        line(
            "outside-roots",
            "~/other/app/node_modules",
            "to allow it: rosie roots add ~/other/app/node_modules"
        )
    );
}

#[test]
fn rules_disagreeing_on_tier_are_a_conflict_naming_each_side() {
    let sandbox = Sandbox::new();
    sandbox.add_rules("deps.toml", AGGRESSIVE_NODE_RULE);
    sandbox.node_project(&format!("{CWD}/api"));

    let ran = check(&sandbox, &["api/node_modules"]);

    ran.assert_status(ExitStatus::Failed);
    assert_eq!(
        line_for(&ran, "~/src/api/node_modules"),
        line(
            "conflict",
            "~/src/api/node_modules",
            "rosie/node-modules (normal) vs rosie/deps (aggressive); choose clean or keep in \
             the plan"
        )
    );
}

// a folder containing targets

#[test]
fn a_folder_containing_targets_is_summarized_with_its_most_consequential_status() {
    let sandbox = Sandbox::new();
    sandbox.add_rules("venv.toml", VENV_RULE);
    sandbox.node_project(&format!("{CWD}/web/a"));
    sandbox.node_project(&format!("{CWD}/web/b"));
    sandbox
        .fake
        .add_file(format!("{CWD}/web/c/pyproject.toml"), "");
    sandbox
        .fake
        .add_sized_file(format!("{CWD}/web/c/.venv/lib"), 210_000_000);

    let ran = check(&sandbox, &["web"]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(
        line_for(&ran, "~/src/web"),
        line(
            "clean",
            "~/src/web",
            "contains 3 items: 2 clean (8.2 KB), 1 keep (210.0 MB); see rosie scan tree \
             ~/src/web"
        )
    );
}

#[test]
fn a_folder_whose_targets_are_all_keep_is_keep() {
    let sandbox = Sandbox::new();
    sandbox.add_rules("venv.toml", VENV_RULE);
    sandbox
        .fake
        .add_file(format!("{CWD}/old/pyproject.toml"), "");
    sandbox
        .fake
        .add_file(format!("{CWD}/old/.venv/bin/python"), "x");

    let ran = check(&sandbox, &["old"]);

    ran.assert_status(ExitStatus::Failed);
    assert_eq!(line_for(&ran, "~/src/old").status, "keep");
}

/// A folder with no plan entry inside is never `no rule` when something inside it was
/// refused or skipped.
#[test]
fn a_folder_holding_only_refused_or_skipped_items_says_what_is_inside() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/busy"));
    sandbox
        .fake
        .respond_running(&[(77, &format!("{CWD}/busy/node_modules/.bin/vite"))]);
    sandbox.fake.add_dir(format!("{CWD}/apps/Foo.app"));

    let ran = check(&sandbox, &["busy", "apps"]);

    ran.assert_status(ExitStatus::Failed);
    assert_eq!(
        line_for(&ran, "~/src/busy"),
        line(
            "in use",
            "~/src/busy",
            "contains 1 in use; see rosie scan tree ~/src/busy"
        )
    );
    assert_eq!(
        line_for(&ran, "~/src/apps"),
        line(
            "skipped",
            "~/src/apps",
            "contains 1 skipped; see rosie scan tree ~/src/apps"
        )
    );
}

/// A link out of the roots is skipped where it sits in the folder, though the skip names
/// the folder it leads to.
#[test]
fn a_folder_holding_a_link_out_of_the_roots_counts_the_skip() {
    let sandbox = Sandbox::with_roots(&[CWD]);
    sandbox.fake.add_dir(format!("{HOME}/elsewhere"));
    sandbox
        .fake
        .add_symlink(format!("{CWD}/proj/linked"), format!("{HOME}/elsewhere"));

    let ran = check(&sandbox, &["proj", "--enter-links=all"]);

    ran.assert_status(ExitStatus::Failed);
    assert_eq!(
        line_for(&ran, "~/src/proj"),
        line(
            "skipped",
            "~/src/proj",
            "contains 1 skipped; see rosie scan tree ~/src/proj"
        )
    );
}

// scan report words otherwise

#[test]
fn an_item_a_process_runs_from_is_in_use_naming_the_program_and_pid() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));
    sandbox
        .fake
        .respond_running(&[(4123, &format!("{CWD}/app/node_modules/.bin/vite"))]);

    let ran = check(&sandbox, &["app/node_modules"]);

    ran.assert_status(ExitStatus::Failed);
    assert_eq!(
        line_for(&ran, "~/src/app/node_modules"),
        line(
            "in use",
            "~/src/app/node_modules",
            "running: vite (pid 4123)"
        )
    );
}

#[test]
fn a_typed_path_through_a_symlink_is_skipped_naming_the_link_and_its_target() {
    let sandbox = Sandbox::new();
    sandbox
        .fake
        .add_dir(format!("{HOME}/dotfiles/cargo/registry"));
    sandbox
        .fake
        .add_symlink(format!("{HOME}/.cargo"), "dotfiles/cargo");

    let ran = check(&sandbox, &[&format!("{HOME}/.cargo/registry")]);

    ran.assert_status(ExitStatus::Failed);
    assert_eq!(
        line_for(&ran, "~/.cargo/registry"),
        line(
            "skipped",
            "~/.cargo/registry",
            "through symlink ~/.cargo → ~/dotfiles/cargo; enter_links = \"roots\" allows it"
        )
    );
}

#[test]
fn a_path_on_another_volume_is_skipped_naming_the_flag_that_enters_it() {
    let sandbox = Sandbox::new();
    sandbox.fake.add_dir("/Volumes/Backup/build");
    sandbox.fake.mount("/Volumes/Backup", 2);

    let closed = check(&sandbox, &["/Volumes/Backup/build"]);
    let entered = check(&sandbox, &["/Volumes/Backup/build", "--enter-mounts"]);

    closed.assert_status(ExitStatus::Failed);
    assert_eq!(
        line_for(&closed, "/Volumes/Backup/build"),
        line(
            "skipped",
            "/Volumes/Backup/build",
            "another volume; --enter-mounts enters it"
        )
    );
    assert_eq!(
        line_for(&entered, "/Volumes/Backup/build").status,
        "no rule"
    );
}

#[test]
fn a_folder_closed_several_ways_names_every_flag_that_enters_it() {
    let sandbox = Sandbox::new();
    sandbox.fake.add_dir(format!("{CWD}/Old.app/Contents"));
    sandbox.fake.mount(format!("{CWD}/Old.app"), 2);
    sandbox.fake.make_dataless(format!("{CWD}/Old.app"));

    let ran = check(&sandbox, &["Old.app/Contents"]);

    assert_eq!(
        line_for(&ran, "~/src/Old.app/Contents").reason,
        "cloud placeholder, not downloaded; another volume; bundle; --enter-placeholders, \
         --enter-mounts and --enter-bundles enter it"
    );
}

#[test]
fn a_path_no_rule_reaches_is_no_rule_and_a_missing_one_does_not_exist() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));
    sandbox.fake.add_file(format!("{CWD}/app/src/main.js"), "x");

    let ran = check(&sandbox, &["app/src", &format!("{HOME}/Downloads/missing")]);

    ran.assert_status(ExitStatus::Failed);
    assert_eq!(
        line_for(&ran, "~/src/app/src"),
        line(
            "no rule",
            "~/src/app/src",
            "no rule matches it or anything inside"
        )
    );
    assert_eq!(
        line_for(&ran, "~/Downloads/missing"),
        line("no rule", "~/Downloads/missing", "does not exist")
    );
}

/// The walk never reached a path below a folder it could not list, so the path reads
/// `skipped` with the listing's error, never `no rule`.
#[test]
fn a_path_below_a_folder_that_cannot_be_listed_is_skipped_with_the_error() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/locked/app"));
    sandbox
        .fake
        .fail_on(format!("{CWD}/locked"), Op::ReadDir, ErrorKind::Other);

    let ran = check(&sandbox, &["locked/app/node_modules", "locked"]);

    ran.assert_status(ExitStatus::Failed);
    assert_eq!(
        line_for(&ran, "~/src/locked/app/node_modules"),
        line(
            "skipped",
            "~/src/locked/app/node_modules",
            "cannot list /Users/me/src/locked: other error"
        )
    );
    assert_eq!(
        line_for(&ran, "~/src/locked"),
        line(
            "skipped",
            "~/src/locked",
            "cannot list /Users/me/src/locked: other error"
        )
    );
}

/// A folder that cannot be listed inside a typed folder with no plan entry counts as
/// skipped there, so the typed folder is never `no rule`.
#[test]
fn a_folder_holding_one_that_cannot_be_listed_counts_it_as_skipped() {
    let sandbox = Sandbox::new();
    sandbox
        .fake
        .add_file(format!("{CWD}/locked/notes.txt"), "x");
    sandbox
        .fake
        .fail_on(format!("{CWD}/locked"), Op::ReadDir, ErrorKind::Other);

    let ran = check(&sandbox, &[CWD]);

    ran.assert_status(ExitStatus::Failed);
    assert_eq!(
        line_for(&ran, "~/src"),
        line(
            "skipped",
            "~/src",
            "contains 1 skipped; see rosie scan tree ~/src"
        )
    );
}

// an app bundle is judged as `rosie scan app` plans it

const SLACK: &str = "/Users/me/Applications/Slack.app";

/// An installed app with no leftovers, which `scan app` plans as one `clean` bundle.
fn install_slack(sandbox: &Sandbox) {
    let plist = format!("{SLACK}/Contents/Info.plist");
    sandbox.fake.add_file(&plist, "<plist/>");
    sandbox
        .fake
        .add_file(format!("{SLACK}/Contents/MacOS/Slack"), "x");
    sandbox.respond(
        extract(&plist, "CFBundleIdentifier"),
        "com.tinyspeck.slackmacgap\n",
    );
    sandbox.respond(extract(&plist, "CFBundleName"), "Slack\n");
    sandbox.respond(Argv::new("/usr/sbin/pkgutil").arg("--pkgs"), "");
}

fn extract(plist: &str, key: &str) -> Argv {
    Argv::new("/usr/bin/plutil")
        .arg("-extract")
        .arg(key)
        .arg("raw")
        .arg("-o")
        .arg("-")
        .arg(plist)
}

#[test]
fn an_app_not_running_takes_its_status_in_the_app_plan() {
    let sandbox = Sandbox::new();
    install_slack(&sandbox);

    let ran = check(&sandbox, &[SLACK]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(
        ran.stdout,
        "clean  ~/Applications/Slack.app  app/com.tinyspeck.slackmacgap (normal)\n"
    );
}

#[test]
fn a_running_app_is_in_use_naming_the_program_and_pid() {
    let sandbox = Sandbox::new();
    install_slack(&sandbox);
    sandbox
        .fake
        .respond_running(&[(4123, &format!("{SLACK}/Contents/MacOS/Slack"))]);

    let ran = check(&sandbox, &[SLACK]);

    ran.assert_status(ExitStatus::Failed);
    assert_eq!(
        line_for(&ran, "~/Applications/Slack.app"),
        line(
            "in use",
            "~/Applications/Slack.app",
            "running: Slack (pid 4123)"
        )
    );
}

/// App mode judges a bundle only when the tree walk claimed nothing above it.
#[test]
fn an_app_inside_a_plan_entry_takes_the_entry_s_status() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));
    sandbox.fake.add_file(
        format!("{CWD}/app/node_modules/Tool.app/Contents/Info.plist"),
        "",
    );

    let ran = check(&sandbox, &["app/node_modules/Tool.app"]);

    assert_eq!(
        line_for(&ran, "~/src/app/node_modules/Tool.app").reason,
        "inside ~/src/app/node_modules (rosie/node-modules)"
    );
}

/// App mode refusing the bundle is a refusal of the typed path: `skipped` with the
/// refusal's own text, and the other paths still print.
#[test]
fn an_app_outside_the_roots_is_skipped_with_the_refusal() {
    let sandbox = Sandbox::with_roots(&[CWD]);
    sandbox.node_project(&format!("{CWD}/app"));
    install_slack(&sandbox);

    let ran = check(&sandbox, &["app/node_modules", SLACK]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(line_for(&ran, "~/src/app/node_modules").status, "clean");
    assert_eq!(
        line_for(&ran, "~/Applications/Slack.app"),
        line(
            "skipped",
            "~/Applications/Slack.app",
            "cannot clean /Users/me/Applications/Slack.app: \
             /Users/me/Applications/Slack.app is outside the allowed roots"
        )
    );
}

/// A folder merely named `.app` that app mode cannot read is `skipped` with the error,
/// and the other paths still print.
#[test]
fn an_app_without_an_info_plist_is_skipped_with_the_error() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));
    sandbox.fake.add_dir(format!("{CWD}/apps/Foo.app"));

    let ran = check(&sandbox, &["app/node_modules", "apps/Foo.app"]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(line_for(&ran, "~/src/app/node_modules").status, "clean");
    assert_eq!(
        line_for(&ran, "~/src/apps/Foo.app"),
        line(
            "skipped",
            "~/src/apps/Foo.app",
            "/Users/me/src/apps/Foo.app has no Info.plist: neither Contents/Info.plist \
             nor a single Wrapper/<name>.app/Info.plist"
        )
    );
}

// one line per path, readable columns, exit status

#[test]
fn prints_one_line_per_path_in_columns_as_wide_as_the_widest_entry() {
    let sandbox = Sandbox::with_roots(&[CWD]);
    sandbox.node_project(&format!("{CWD}/app"));
    sandbox.node_project(&format!("{HOME}/x"));

    let ran = check(
        &sandbox,
        &[
            "app/node_modules",
            &format!("{HOME}/x/node_modules"),
            "app/src",
        ],
    );

    ran.assert_status(ExitStatus::Success);
    assert_eq!(
        ran.stdout,
        "clean          ~/src/app/node_modules  rosie/node-modules (normal)\n\
         outside-roots  ~/x/node_modules        to allow it: rosie roots add ~/x/node_modules\n\
         no rule        ~/src/app/src           does not exist\n"
    );
}

#[test]
fn exits_zero_only_when_some_path_would_be_cleaned() {
    let sandbox = Sandbox::new();
    sandbox.node_project(&format!("{CWD}/app"));

    let some = check(&sandbox, &["app/node_modules", "app"]);
    let none = check(&sandbox, &["app/package.json"]);

    some.assert_status(ExitStatus::Success);
    none.assert_status(ExitStatus::Failed);
    assert_eq!(line_for(&none, "~/src/app/package.json").status, "no rule");
}

#[test]
fn check_takes_no_sh_flag() {
    let sandbox = Sandbox::new();

    let ran = check(&sandbox, &[".", "--sh"]);

    ran.assert_status(ExitStatus::Usage);
}

// the verdict equals the plan's

#[test]
fn each_path_s_status_equals_its_status_in_the_tree_plan() {
    let sandbox = Sandbox::with_roots(&[&format!("{CWD}/in")]);
    sandbox.add_rules("venv.toml", VENV_RULE);
    sandbox.add_rules("deps.toml", AGGRESSIVE_NODE_RULE);
    sandbox.add_rules("cargo.toml", cli_fixture::CARGO_RULE);
    sandbox.node_project(&format!("{CWD}/in/web"));
    sandbox.node_project(&format!("{CWD}/out/web"));
    sandbox
        .fake
        .add_file(format!("{CWD}/in/py/pyproject.toml"), "");
    sandbox.fake.add_file(format!("{CWD}/in/py/.venv/x"), "x");
    sandbox.fake.add_file(format!("{CWD}/in/rs/Cargo.toml"), "");
    sandbox
        .fake
        .add_file(format!("{CWD}/in/rs/target/debug/x"), "x");

    let scanned = sandbox.run(&["scan", "tree", "."]);
    let planned = planned_statuses(&scanned.stdout);
    let paths: Vec<&str> = planned.iter().map(|(path, _)| path.as_str()).collect();
    let checked = check(&sandbox, &paths);

    assert_eq!(planned.len(), 4, "{}", scanned.stdout);
    for (path, status) in &planned {
        let shown = path.replacen(HOME, "~", 1);
        assert_eq!(&line_for(&checked, &shown).status, status, "{path}");
    }
}

/// Each delete entry's path and status in a TOML plan.
fn planned_statuses(plan: &str) -> Vec<(String, String)> {
    let plan: toml::Table = plan.parse().expect("a TOML plan");
    let deletes = plan
        .get("delete")
        .and_then(toml::Value::as_array)
        .expect("delete entries");
    deletes
        .iter()
        .map(|delete| {
            let field = |key: &str| delete[key].as_str().expect("a string field").to_owned();
            (field("path"), field("status"))
        })
        .collect()
}
