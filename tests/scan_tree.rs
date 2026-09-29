//! Fixture tier for `tree` scans: the walk, its skips, matching, sizing, and item marks
//! on the in-memory fake (`docs/spec/performance.md#scan`, `docs/spec/safety.md`).

mod scan_fixture;

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use rosie::fs::fake::{Call, FakeBackend};
use rosie::fs::{self, Op, ROOT_UID};
use rosie::plan::{AggressiveItems, RunAs, Status, WalkSkip};
use rosie::scan::{Error, NoProgress};
use scan_fixture::*;

const PYCACHE: &str = "[rules.pycache]\nstrategy = \"name\"\ntarget = \"__pycache__\"\n";
const VENV: &str = "[rules.venv]\nstrategy = \"marker\"\ntarget_aggressive = \".venv\"\ninside = [\"pyvenv.cfg\"]\n";
const GRADLE: &str = "[rules.gradle-build]\nstrategy = \"lua\"\ntarget = \"build\"\nexpr = \"exists(parent(path) .. '/build.gradle')\"\n";

fn listed(fake: &FakeBackend, path: &str) -> bool {
    fake.calls().contains(&Call::ReadDir(PathBuf::from(path)))
}

fn walk_flags(configure: impl FnOnce(&mut rosie::config::Walk)) -> Options {
    let mut options = Options::default();
    configure(&mut options.walk);
    options
}

// matching and descent

#[test]
fn matches_marker_rules_and_stops_descending_at_a_match() {
    let fake = node_home();
    add_rules(&fake, "py.toml", PYCACHE);
    node_project(&fake, "/Users/me/code/app");
    fake.add_file("/Users/me/code/app/node_modules/b/package.json", "{}");
    fake.add_file("/Users/me/code/app/node_modules/b/node_modules/c/x.js", "x");
    fake.add_file("/Users/me/code/app/node_modules/d/__pycache__/m.pyc", "x");
    fake.add_file("/Users/me/code/lib/node_modules/x.js", "x");

    let scan = tree(&fake, CODE);

    assert_eq!(
        entries(&scan),
        [(
            "/Users/me/code/app/node_modules",
            vec!["rosie/node-modules"],
            Status::Ticked
        )]
    );
    assert!(scan.problems.is_empty(), "{:?}", problems(&scan));
}

#[test]
fn a_normal_match_inside_an_aggressive_match_is_not_offered() {
    let fake = fake_home();
    add_rules(&fake, "py.toml", &format!("{PYCACHE}\n{VENV}"));
    fake.add_file("/Users/me/code/py/.venv/pyvenv.cfg", "home = /usr");
    fake.add_file("/Users/me/code/py/.venv/lib/__pycache__/m.pyc", "x");

    let unticked = tree(&fake, CODE);
    let ticked = tree_with(
        &fake,
        CODE,
        Options {
            aggressive: AggressiveItems::Ticked,
            ..Options::default()
        },
    );

    let venv = "/Users/me/code/py/.venv";
    assert_eq!(
        entries(&unticked),
        [(venv, vec!["rosie/venv"], Status::Unticked)]
    );
    assert_eq!(
        entries(&ticked),
        [(venv, vec!["rosie/venv"], Status::Ticked)]
    );
}

#[test]
fn inside_markers_are_checked_by_listing_the_candidate() {
    let fake = fake_home();
    add_rules(&fake, "py.toml", VENV);
    fake.add_file("/Users/me/code/py/.venv/pyvenv.cfg", "home = /usr");
    fake.add_file("/Users/me/code/other/.venv/bin/python", "x");

    let scan = tree(&fake, CODE);

    assert_eq!(paths(&scan), ["/Users/me/code/py/.venv"]);
}

#[test]
fn lua_rules_run_on_name_matched_candidates_through_the_walk() {
    let fake = fake_home();
    add_rules(&fake, "gradle.toml", GRADLE);
    fake.add_file("/Users/me/code/app/build.gradle", "");
    fake.add_file("/Users/me/code/app/build/out.jar", "x");
    fake.add_file("/Users/me/code/docs/build/index.html", "x");

    let scan = tree(&fake, CODE);

    assert_eq!(paths(&scan), ["/Users/me/code/app/build"]);
}

#[test]
fn a_failing_lua_rule_is_reported_and_its_candidate_does_not_match() {
    let fake = fake_home();
    add_rules(
        &fake,
        "broken.toml",
        "[rules.broken]\nstrategy = \"lua\"\ntarget = \"build\"\n\
         expr = \"read_json(parent(path) .. '/missing.json').x\"\n",
    );
    fake.add_file("/Users/me/code/app/build/out.jar", "x");

    let scan = tree(&fake, CODE);

    assert!(paths(&scan).is_empty());
    let [problem] = problems(&scan).try_into().expect("one problem");
    assert!(problem.contains("rosie/broken"), "{problem}");
}

#[test]
fn symlinks_are_never_entered_or_matched() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_file("/Users/me/elsewhere/nm/index.js", "x");
    fake.add_symlink("/Users/me/code/app/node_modules", "/Users/me/elsewhere/nm");
    node_project(&fake, "/Users/me/other/proj");
    fake.add_symlink("/Users/me/code/linked", "/Users/me/other");

    let scan = tree(&fake, CODE);

    assert!(paths(&scan).is_empty(), "{:?}", paths(&scan));
    assert!(!listed(&fake, "/Users/me/code/linked"));
    assert!(!listed(&fake, "/Users/me/code/linked/proj"));
    assert!(!listed(&fake, "/Users/me/code/app/node_modules"));
}

#[test]
fn the_start_folder_is_never_itself_a_target() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/app");
    fake.add_file("/Users/me/code/app/node_modules/b/package.json", "{}");
    fake.add_file("/Users/me/code/app/node_modules/b/node_modules/c.js", "x");

    let scan = tree(&fake, "/Users/me/code/app/node_modules");

    assert_eq!(
        paths(&scan),
        ["/Users/me/code/app/node_modules/b/node_modules"]
    );
}

// the start folder

#[test]
fn the_start_folder_is_checked_as_typed() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/app");
    fake.add_symlink("/Users/me/src", "/Users/me/code");
    let scanner = scanner(&fake, Options::default());

    let through_link = scanner.tree(Path::new("/Users/me/src"), &NoProgress);
    let misspelled = scanner.tree(Path::new("/Users/me/Code"), &NoProgress);
    let a_file = scanner.tree(Path::new("/Users/me/code/app/package.json"), &NoProgress);

    assert!(
        matches!(&through_link, Err(Error::Fs(fs::Error::Symlink { link, .. })) if link == Path::new("/Users/me/src")),
        "{through_link:?}"
    );
    assert!(
        matches!(&misspelled, Err(Error::Fs(fs::Error::Misspelled { real, .. })) if real == Path::new(CODE)),
        "{misspelled:?}"
    );
    assert!(
        matches!(&a_file, Err(Error::NotAFolder { .. })),
        "{a_file:?}"
    );
}

#[test]
fn a_start_folder_that_is_a_placeholder_is_not_opened_unless_enter_placeholders() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/app");
    fake.make_dataless(CODE);

    let closed = tree(&fake, CODE);
    let closed_listed = listed(&fake, CODE);
    let opened = tree_with(
        &fake,
        CODE,
        walk_flags(|walk| walk.enter_placeholders = true),
    );

    assert!(!closed_listed);
    assert!(paths(&closed).is_empty());
    assert_eq!(skipped(&closed), [(CODE, WalkSkip::Placeholder)]);
    assert_eq!(paths(&opened), ["/Users/me/code/app/node_modules"]);
}

// walk skips

#[test]
fn bundles_are_not_entered_unless_enter_bundles() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/Tool.app/Contents/Resources");

    let closed = tree(&fake, CODE);
    let entered = tree_with(&fake, CODE, walk_flags(|walk| walk.enter_bundles = true));

    assert!(paths(&closed).is_empty());
    assert_eq!(
        skipped(&closed),
        [("/Users/me/code/Tool.app", WalkSkip::Bundle)]
    );
    assert_eq!(closed.skip_counts().bundles, 1);
    assert_eq!(
        paths(&entered),
        ["/Users/me/code/Tool.app/Contents/Resources/node_modules"]
    );
    assert!(entered.skipped.is_empty());
}

#[test]
fn every_bundle_extension_is_skipped() {
    let fake = node_home();
    let extensions = [
        "app",
        "framework",
        "bundle",
        "plugin",
        "xcarchive",
        "photoslibrary",
        "sparsebundle",
        "dSYM",
    ];
    for extension in extensions {
        node_project(&fake, &format!("/Users/me/code/X.{extension}"));
    }
    node_project(&fake, "/Users/me/code/X.apps");

    let scan = tree(&fake, CODE);

    assert_eq!(scan.skip_counts().bundles, extensions.len());
    assert_eq!(paths(&scan), ["/Users/me/code/X.apps/node_modules"]);
}

#[test]
fn placeholders_are_not_opened_unless_enter_placeholders() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/cloud/app");
    fake.make_dataless("/Users/me/code/cloud");

    let closed = tree(&fake, CODE);
    let closed_listed = listed(&fake, "/Users/me/code/cloud");
    let opened = tree_with(
        &fake,
        CODE,
        walk_flags(|walk| walk.enter_placeholders = true),
    );

    assert!(paths(&closed).is_empty());
    assert!(!closed_listed);
    assert_eq!(
        skipped(&closed),
        [("/Users/me/code/cloud", WalkSkip::Placeholder)]
    );
    assert_eq!(paths(&opened), ["/Users/me/code/cloud/app/node_modules"]);
}

#[test]
fn a_placeholder_is_never_a_target() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/app");
    fake.make_dataless("/Users/me/code/app/node_modules");

    let closed = tree(&fake, CODE);
    let opened = tree_with(
        &fake,
        CODE,
        walk_flags(|walk| walk.enter_placeholders = true),
    );

    assert!(paths(&closed).is_empty());
    assert_eq!(
        skipped(&closed),
        [("/Users/me/code/app/node_modules", WalkSkip::Placeholder)]
    );
    assert!(paths(&opened).is_empty());
}

#[test]
fn a_denied_folder_mid_walk_is_logged_as_a_skip_and_the_walk_goes_on() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/private/app");
    node_project(&fake, "/Users/me/code/open/app");
    fake.fail_on(
        "/Users/me/code/private",
        Op::ReadDir,
        ErrorKind::PermissionDenied,
    );

    let scan = tree(&fake, CODE);

    assert_eq!(paths(&scan), ["/Users/me/code/open/app/node_modules"]);
    assert_eq!(
        skipped(&scan),
        [("/Users/me/code/private", WalkSkip::Denied)]
    );
    assert_eq!(scan.skip_counts().denied, 1);
    assert!(scan.problems.is_empty());
}

#[test]
fn a_denied_entry_lstat_is_a_skip_too() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/app");
    fake.fail_on(
        "/Users/me/code/app/node_modules",
        Op::Lstat,
        ErrorKind::PermissionDenied,
    );

    let scan = tree(&fake, CODE);

    assert!(paths(&scan).is_empty());
    assert_eq!(
        skipped(&scan),
        [("/Users/me/code/app/node_modules", WalkSkip::Denied)]
    );
}

#[test]
fn other_walk_errors_are_problems_and_the_walk_goes_on() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/broken/app");
    node_project(&fake, "/Users/me/code/fine/app");
    fake.fail_on("/Users/me/code/broken", Op::ReadDir, ErrorKind::InvalidData);

    let scan = tree(&fake, CODE);

    assert_eq!(paths(&scan), ["/Users/me/code/fine/app/node_modules"]);
    let [problem] = problems(&scan).try_into().expect("one problem");
    assert!(problem.contains("/Users/me/code/broken"), "{problem}");
    assert!(scan.skipped.is_empty());
}

// sizing

#[test]
fn sizes_are_allocated_bytes_of_the_whole_target() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/a/index.js", 8192);
    fake.add_sized_file("/Users/me/code/app/node_modules/a/b/c/deep.js", 4096);
    fake.add_sized_file("/Users/me/code/app/node_modules/top.js", 4096);

    let scan = tree(&fake, CODE);

    assert_eq!(size_of(&scan, "/Users/me/code/app/node_modules"), 16_384);
}

#[test]
fn hardlinks_are_counted_once_across_the_whole_plan() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_file("/Users/me/code/lib/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/a.js", 10_000);
    fake.add_hardlink(
        "/Users/me/code/app/node_modules/a.js",
        "/Users/me/code/app/node_modules/again.js",
    );
    fake.add_hardlink(
        "/Users/me/code/app/node_modules/a.js",
        "/Users/me/code/lib/node_modules/a.js",
    );
    fake.add_sized_file("/Users/me/code/lib/node_modules/own.js", 500);

    let scan = tree(&fake, CODE);

    let app = size_of(&scan, "/Users/me/code/app/node_modules");
    let lib = size_of(&scan, "/Users/me/code/lib/node_modules");
    assert_eq!(app + lib, 10_500);
    assert!(lib == 500 || lib == 10_500, "lib {lib}");
}

#[test]
fn a_file_with_two_names_in_two_targets_is_counted_once() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_file("/Users/me/code/lib/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/a.js", 10_000);
    fake.add_hardlink(
        "/Users/me/code/app/node_modules/a.js",
        "/Users/me/code/lib/node_modules/a.js",
    );

    let scan = tree(&fake, CODE);

    let app = size_of(&scan, "/Users/me/code/app/node_modules");
    let lib = size_of(&scan, "/Users/me/code/lib/node_modules");
    assert_eq!(app + lib, 10_000);
}

#[test]
fn huge_sizes_add_up_exactly_and_saturate_rather_than_wrap() {
    let fake = node_home();
    let petabytes_3 = 3_000_000_000_000_000;
    fake.add_file("/Users/me/code/big/package.json", "{}");
    for name in ["a", "b", "c"] {
        fake.add_sized_file(
            format!("/Users/me/code/big/node_modules/{name}"),
            petabytes_3,
        );
    }
    fake.add_file("/Users/me/code/max/package.json", "{}");
    for name in ["a", "b"] {
        fake.add_sized_file(
            format!("/Users/me/code/max/node_modules/{name}"),
            u64::MAX / 2 + 1,
        );
    }

    let scan = tree(&fake, CODE);

    assert_eq!(
        size_of(&scan, "/Users/me/code/big/node_modules"),
        3 * petabytes_3
    );
    assert_eq!(size_of(&scan, "/Users/me/code/max/node_modules"), u64::MAX);
}

#[test]
fn a_target_total_saturates_when_its_folders_together_overflow() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/a", u64::MAX / 2 + 1);
    fake.add_sized_file("/Users/me/code/app/node_modules/sub/b", u64::MAX / 2 + 1);

    let scan = tree(&fake, CODE);

    assert_eq!(size_of(&scan, "/Users/me/code/app/node_modules"), u64::MAX);
}

#[test]
fn sizing_leaves_out_other_volumes_and_placeholders_inside_a_target() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/ok.js", 100);
    fake.add_sized_file("/Users/me/code/app/node_modules/mnt/huge", 1_000_000);
    fake.mount("/Users/me/code/app/node_modules/mnt", 9);
    fake.add_sized_file("/Users/me/code/app/node_modules/cloud.bin", 50_000);
    fake.make_dataless("/Users/me/code/app/node_modules/cloud.bin");

    let scan = tree(&fake, CODE);

    assert_eq!(size_of(&scan, "/Users/me/code/app/node_modules"), 100);
    assert_eq!(
        skipped(&scan),
        [
            (
                "/Users/me/code/app/node_modules/cloud.bin",
                WalkSkip::Placeholder
            ),
            ("/Users/me/code/app/node_modules/mnt", WalkSkip::Mount),
        ]
    );
    assert!(!listed(&fake, "/Users/me/code/app/node_modules/mnt"));
}

#[test]
fn sizing_goes_into_bundles_inside_a_target() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/Tool.app/Contents/bin", 700);

    let scan = tree(&fake, CODE);

    assert_eq!(size_of(&scan, "/Users/me/code/app/node_modules"), 700);
    assert!(scan.skipped.is_empty());
}

#[test]
fn a_denied_folder_inside_a_target_is_a_skip_and_the_rest_is_sized() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/ok.js", 100);
    fake.add_sized_file("/Users/me/code/app/node_modules/locked/x.js", 900);
    fake.fail_on(
        "/Users/me/code/app/node_modules/locked",
        Op::ReadDir,
        ErrorKind::PermissionDenied,
    );

    let scan = tree(&fake, CODE);

    assert_eq!(size_of(&scan, "/Users/me/code/app/node_modules"), 100);
    assert_eq!(
        skipped(&scan),
        [("/Users/me/code/app/node_modules/locked", WalkSkip::Denied)]
    );
}

#[test]
fn a_denied_entry_inside_a_target_is_a_skip_and_the_rest_is_sized() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_sized_file("/Users/me/code/app/node_modules/ok.js", 100);
    fake.add_sized_file("/Users/me/code/app/node_modules/secret.js", 900);
    fake.fail_on(
        "/Users/me/code/app/node_modules/secret.js",
        Op::Lstat,
        ErrorKind::PermissionDenied,
    );

    let scan = tree(&fake, CODE);

    assert_eq!(size_of(&scan, "/Users/me/code/app/node_modules"), 100);
    assert_eq!(
        skipped(&scan),
        [(
            "/Users/me/code/app/node_modules/secret.js",
            WalkSkip::Denied
        )]
    );
}

#[test]
fn progress_counts_every_target_and_byte() {
    let fake = node_home();
    fake.add_file("/Users/me/code/a/package.json", "{}");
    fake.add_sized_file("/Users/me/code/a/node_modules/x", 3000);
    fake.add_file("/Users/me/code/b/package.json", "{}");
    fake.add_sized_file("/Users/me/code/b/node_modules/y", 5000);
    let progress = Recording::default();

    let scan = scanner(&fake, Options::default())
        .tree(Path::new(CODE), &progress)
        .expect("scan runs");

    assert_eq!(paths(&scan).len(), 2);
    assert_eq!(progress.found.into_inner(), 2);
    assert_eq!(progress.bytes.into_inner(), 8000);
}

#[test]
fn sizing_reports_progress_once_per_folder_not_per_entry() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    for index in 0..40 {
        fake.add_sized_file(format!("/Users/me/code/app/node_modules/f{index}"), 100);
        fake.add_sized_file(format!("/Users/me/code/app/node_modules/a/f{index}"), 100);
    }
    fake.add_dir("/Users/me/code/app/node_modules/empty");
    let progress = Recording::default();

    let scan = scanner(&fake, Options::default())
        .tree(Path::new(CODE), &progress)
        .expect("scan runs");

    assert_eq!(size_of(&scan, "/Users/me/code/app/node_modules"), 8000);
    assert_eq!(progress.bytes.into_inner(), 8000);
    // One report for the target itself, then at most one per folder in it.
    let calls = progress.sized_calls.into_inner();
    assert!((1..=4).contains(&calls), "{calls} progress reports");
}

// marks

#[test]
fn targets_outside_the_roots_are_blocked() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/app");
    node_project(&fake, "/Users/me/code/lib");
    let options = Options {
        roots: vec![PathBuf::from("/Users/me/code/app")],
        ..Options::default()
    };

    let scan = tree_with(&fake, CODE, options);

    let statuses: Vec<(&str, Status)> = entries(&scan)
        .into_iter()
        .map(|(path, _, status)| (path, status))
        .collect();
    assert_eq!(
        statuses,
        [
            ("/Users/me/code/app/node_modules", Status::Ticked),
            ("/Users/me/code/lib/node_modules", Status::Blocked),
        ]
    );
}

#[test]
fn targets_not_owned_by_the_user_are_marked_sudo() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/app");
    node_project(&fake, "/Users/me/code/lib");
    fake.chown("/Users/me/code/lib/node_modules", ROOT_UID);

    let scan = tree(&fake, CODE);

    let run_as: Vec<RunAs> = scan.plan.deletes().iter().map(|d| d.run_as).collect();
    assert_eq!(run_as, [RunAs::User, RunAs::Sudo]);
}

// fixed paths under the folder

const CACHE_PATHS: &str = "[rules.npm-cache]\nstrategy = \"path\"\npaths = [\"~/.npm\"]\n\n\
    [rules.system-cache]\nstrategy = \"path\"\npaths = [\"/Library/Caches/x\"]\n\n\
    [rules.docker]\nstrategy = \"tool\"\ncmd = \"docker system prune --force\"\n";

#[test]
fn includes_fixed_rule_paths_under_the_folder_only() {
    let fake = fake_home();
    add_rules(&fake, "caches.toml", CACHE_PATHS);
    fake.add_sized_file("/Users/me/.npm/_cacache/index", 4096);
    fake.add_file("/Real/Caches/x/f", "x");
    fake.add_symlink("/Library", "/Real");

    let scan = tree(&fake, HOME);

    assert_eq!(
        entries(&scan),
        [("/Users/me/.npm", vec!["rosie/npm-cache"], Status::Ticked)]
    );
    assert_eq!(size_of(&scan, "/Users/me/.npm"), 4096);
    assert!(scan.plan.tools().is_empty());
    assert!(scan.problems.is_empty(), "{:?}", problems(&scan));
}

#[test]
fn an_absent_or_denied_fixed_path_is_not_reported_as_a_problem() {
    let fake = fake_home();
    add_rules(
        &fake,
        "caches.toml",
        "[rules.gone-cache]\nstrategy = \"path\"\npaths = [\"~/.gone\"]\n\n\
        [rules.locked-cache]\nstrategy = \"path\"\npaths = [\"~/locked/cache\"]\n",
    );
    fake.add_dir("/Users/me/locked/cache");
    fake.fail_on(
        "/Users/me/locked/cache",
        Op::Lstat,
        ErrorKind::PermissionDenied,
    );

    let scan = tree(&fake, HOME);

    assert!(paths(&scan).is_empty(), "{:?}", paths(&scan));
    assert_eq!(
        skipped(&scan),
        [("/Users/me/locked/cache", WalkSkip::Denied)]
    );
    assert!(scan.problems.is_empty(), "{:?}", problems(&scan));
}

#[test]
fn a_dataless_fixed_file_under_the_folder_is_skipped_and_plain_dataless_files_are_not_logged() {
    let fake = fake_home();
    add_rules(
        &fake,
        "file.toml",
        "[rules.history]\nstrategy = \"path\"\npaths = [\"~/.lesshst\"]\n",
    );
    fake.add_sized_file("/Users/me/.lesshst", 4096);
    fake.make_dataless("/Users/me/.lesshst");
    fake.add_sized_file("/Users/me/Documents/paper.pdf", 4096);
    fake.make_dataless("/Users/me/Documents/paper.pdf");

    let closed = tree(&fake, HOME);
    let opened = tree_with(
        &fake,
        HOME,
        walk_flags(|walk| walk.enter_placeholders = true),
    );

    for scan in [closed, opened] {
        assert!(paths(&scan).is_empty());
        assert_eq!(
            skipped(&scan),
            [("/Users/me/.lesshst", WalkSkip::Placeholder)]
        );
    }
}

#[test]
fn a_fixed_path_that_is_itself_a_symlink_is_reported_once_and_not_planned() {
    let fake = fake_home();
    add_rules(
        &fake,
        "node.toml",
        "[rules.npm-cache]\nstrategy = \"path\"\npaths = [\"~/.npm\"]\n",
    );
    fake.add_file("/Users/me/npm-real/index", "x");
    fake.add_symlink("/Users/me/.npm", "/Users/me/npm-real");

    let scan = tree(&fake, HOME);

    assert!(paths(&scan).is_empty());
    let [problem] = problems(&scan).try_into().expect("one problem");
    assert!(problem.contains("symlink /Users/me/.npm"), "{problem}");
}

#[test]
fn skips_are_listed_in_path_order() {
    let fake = node_home();
    fake.add_dir("/Users/me/code/a/deep/Tool.app");
    fake.add_dir("/Users/me/code/b.app");

    let scan = tree(&fake, CODE);

    assert_eq!(
        skipped(&scan),
        [
            ("/Users/me/code/a/deep/Tool.app", WalkSkip::Bundle),
            ("/Users/me/code/b.app", WalkSkip::Bundle),
        ]
    );
}

#[test]
fn an_entry_that_vanishes_after_listing_is_passed_over() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/app");
    node_project(&fake, "/Users/me/code/gone");
    fake.fail_on("/Users/me/code/gone", Op::Lstat, ErrorKind::NotFound);

    let scan = tree(&fake, CODE);

    assert_eq!(paths(&scan), ["/Users/me/code/app/node_modules"]);
    assert!(scan.problems.is_empty(), "{:?}", problems(&scan));
    assert!(scan.skipped.is_empty());
}

#[test]
fn a_file_named_like_a_folder_target_is_not_matched() {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_file("/Users/me/code/app/node_modules", "not a folder");

    let scan = tree(&fake, CODE);

    assert!(paths(&scan).is_empty(), "{:?}", paths(&scan));
}

#[test]
fn a_fixed_path_and_a_folder_rule_on_one_folder_list_both_rules() {
    let fake = node_home();
    add_rules(
        &fake,
        "fixed.toml",
        "[rules.pinned]\nstrategy = \"path\"\npaths = [\"~/code/app/node_modules\"]\n",
    );
    node_project(&fake, "/Users/me/code/app");

    let scan = tree(&fake, HOME);

    assert_eq!(
        entries(&scan),
        [(
            "/Users/me/code/app/node_modules",
            vec!["rosie/node-modules", "rosie/pinned"],
            Status::Ticked
        )]
    );
}

#[test]
fn a_fixed_file_under_the_folder_is_a_file_target() {
    let fake = fake_home();
    add_rules(
        &fake,
        "file.toml",
        "[rules.history]\nstrategy = \"path\"\npaths = [\"~/.lesshst\"]\n",
    );
    fake.add_sized_file("/Users/me/.lesshst", 4096);

    let scan = tree(&fake, HOME);

    let [delete] = scan.plan.deletes() else {
        panic!("one delete expected: {:?}", scan.plan);
    };
    assert_eq!(delete.path, Path::new("/Users/me/.lesshst"));
    assert_eq!(delete.kind, rosie::plan::ItemKind::File);
    assert_eq!(delete.size.get(), 4096);
}

#[test]
fn a_fixed_path_through_a_symlink_under_the_folder_is_reported() {
    let fake = fake_home();
    add_rules(
        &fake,
        "caches.toml",
        "[rules.tool-cache]\nstrategy = \"path\"\npaths = [\"~/Library/Caches/tool\"]\n",
    );
    fake.add_file("/Users/me/real-caches/tool/f", "x");
    fake.add_symlink("/Users/me/Library/Caches", "/Users/me/real-caches");

    let scan = tree(&fake, HOME);

    assert!(paths(&scan).is_empty(), "{:?}", paths(&scan));
    let [problem] = problems(&scan).try_into().expect("one problem");
    assert!(
        problem.contains("symlink /Users/me/Library/Caches"),
        "{problem}"
    );
}
