//! Fixture tier for `caches` scans: fixed rule paths and tool rules on the in-memory
//! fake (`docs/spec/cli.md#modes`, `docs/spec/rules.md#paths`).

mod scan_fixture;

use std::path::{Path, PathBuf};

use rosie::config::Walk;
use rosie::fs::{EnterLinks, ROOT_UID};
use rosie::plan::{AggressiveItems, ItemKind, RunAs, Selection, Status, WalkSkip};
use scan_fixture::*;

const NPM: &str = "[rules.npm-cache]\nstrategy = \"path\"\npaths = [\"~/.npm\"]\n";

fn npm_home() -> rosie::fs::fake::FakeBackend {
    let fake = fake_home();
    add_rules(&fake, "node.toml", NPM);
    fake.add_sized_file("/Users/me/.npm/_cacache/index", 4096);
    fake
}

fn aggressive() -> Options {
    Options {
        aggressive: AggressiveItems::Clean,
        ..Options::default()
    }
}

fn links(enter_links: EnterLinks) -> Options {
    Options {
        walk: Walk {
            enter_links,
            ..Walk::default()
        },
        ..Options::default()
    }
}

#[test]
fn plans_fixed_paths_with_home_expanded_and_leaves_absent_ones_out() {
    let fake = npm_home();
    add_rules(
        &fake,
        "yarn.toml",
        "[rules.yarn-cache]\nstrategy = \"path\"\npaths = [\"~/Library/Caches/Yarn\"]\n",
    );

    let scan = caches(&fake);

    assert_eq!(
        entries(&scan),
        [("/Users/me/.npm", vec!["rosie/npm-cache"], Status::Clean)]
    );
    assert_eq!(size_of(&scan, "/Users/me/.npm"), 4096);
    assert!(scan.problems.is_empty(), "{:?}", problems(&scan));
    assert!(scan.skipped.is_empty());
}

#[test]
fn two_spellings_of_one_path_are_one_entry_listing_both_rules() {
    let fake = npm_home();
    add_rules(
        &fake,
        "abs.toml",
        "[rules.npm-absolute]\nstrategy = \"path\"\npaths = [\"/Users/me/.npm\"]\n",
    );

    let scan = caches(&fake);

    assert_eq!(
        entries(&scan),
        [(
            "/Users/me/.npm",
            vec!["rosie/npm-absolute", "rosie/npm-cache"],
            Status::Clean
        )]
    );
    assert_eq!(size_of(&scan, "/Users/me/.npm"), 4096);
}

#[test]
fn a_fixed_path_inside_another_collapses_into_it() {
    let fake = fake_home();
    add_rules(
        &fake,
        "caches.toml",
        "[rules.all-caches]\nstrategy = \"path\"\npaths = [\"~/Library/Caches\"]\n\n\
         [rules.pip-cache]\nstrategy = \"path\"\npaths = [\"~/Library/Caches/pip\"]\n",
    );
    fake.add_sized_file("/Users/me/Library/Caches/pip/wheel", 1000);
    fake.add_sized_file("/Users/me/Library/Caches/other", 2000);
    let progress = Recording::default();

    let scan = scanner(&fake, Options::default())
        .caches(&progress)
        .expect("caches scan runs");

    assert_eq!(
        entries(&scan),
        [(
            "/Users/me/Library/Caches",
            vec!["rosie/all-caches"],
            Status::Clean
        )]
    );
    assert_eq!(size_of(&scan, "/Users/me/Library/Caches"), 3000);
    // The inner path is never claimed as a target of its own, so neither the found
    // count nor the bytes see it twice.
    assert_eq!(progress.found.into_inner(), 1);
    assert_eq!(progress.bytes.into_inner(), 3000);
}

#[test]
fn a_fixed_path_through_a_symlink_is_refused_and_reported() {
    let fake = fake_home();
    add_rules(
        &fake,
        "caches.toml",
        "[rules.tool-cache]\nstrategy = \"path\"\npaths = [\"~/Library/Caches/tool\"]\n",
    );
    fake.add_file("/Users/me/real-caches/tool/f", "x");
    fake.add_symlink("/Users/me/Library/Caches", "/Users/me/real-caches");

    let scan = caches(&fake);

    assert!(paths(&scan).is_empty());
    let [problem] = problems(&scan).try_into().expect("one problem");
    assert!(
        problem.contains("passes through the symlink /Users/me/Library/Caches"),
        "{problem}"
    );
}

/// `enter_links = "roots"` (`docs/spec/safety.md#enter_links`).
#[test]
fn a_fixed_path_through_a_symlink_is_planned_at_its_real_location_when_links_are_allowed() {
    let fake = fake_home();
    add_rules(
        &fake,
        "caches.toml",
        "[rules.tool-cache]\nstrategy = \"path\"\npaths = [\"~/Library/Caches/tool\"]\n",
    );
    fake.add_file("/Users/me/real-caches/tool/f", "x");
    fake.add_symlink("/Users/me/Library/Caches", "/Users/me/real-caches");

    let scan = caches_with(&fake, links(EnterLinks::Roots));

    assert_eq!(paths(&scan), ["/Users/me/real-caches/tool"]);
    assert!(scan.problems.is_empty(), "{:?}", problems(&scan));
}

#[test]
fn aggressive_fixed_paths_are_keep_without_the_flag() {
    let fake = fake_home();
    add_rules(
        &fake,
        "jetbrains.toml",
        "[rules.jetbrains]\nstrategy = \"path\"\npaths_aggressive = [\"~/Library/Caches/JetBrains\"]\n",
    );
    fake.add_file("/Users/me/Library/Caches/JetBrains/idx", "x");

    let plain = caches(&fake);
    let flagged = caches_with(&fake, aggressive());

    let path = "/Users/me/Library/Caches/JetBrains";
    assert_eq!(
        entries(&plain),
        [(path, vec!["rosie/jetbrains"], Status::Keep)]
    );
    assert_eq!(
        entries(&flagged),
        [(path, vec!["rosie/jetbrains"], Status::Clean)]
    );
}

#[test]
fn tool_rules_list_their_commands() {
    let fake = fake_home();
    add_rules(
        &fake,
        "docker.toml",
        "[rules.docker]\nstrategy = \"tool\"\ncmd = \"docker system prune --force\"\n\
         cmd_aggressive = \"docker system prune --all --volumes --force\"\n\n\
         [rules.gradle]\nstrategy = \"tool\"\ncmd_aggressive = \"gradle --stop\"\n\n\
         [rules.homebrew]\nstrategy = \"tool\"\ncmd = \"brew cleanup\"\n",
    );

    let tools = |scan: &rosie::scan::Scan| -> Vec<(String, String, Selection)> {
        scan.plan
            .tools()
            .iter()
            .map(|tool| {
                let cmd = tool.command.cmd().to_owned();
                (tool.rule.clone(), cmd, tool.selection)
            })
            .collect()
    };
    let plain = caches(&fake);
    let flagged = caches_with(&fake, aggressive());

    let docker = "rosie/docker".to_owned();
    let gradle = "rosie/gradle".to_owned();
    let homebrew = "rosie/homebrew".to_owned();
    assert_eq!(
        tools(&plain),
        [
            (
                docker.clone(),
                "docker system prune --force".into(),
                Selection::Clean
            ),
            (
                docker.clone(),
                "docker system prune --all --volumes --force".into(),
                Selection::Keep
            ),
            (gradle.clone(), "gradle --stop".into(), Selection::Keep),
            (homebrew.clone(), "brew cleanup".into(), Selection::Clean),
        ]
    );
    assert_eq!(
        tools(&flagged),
        [
            (
                docker,
                "docker system prune --all --volumes --force".into(),
                Selection::Clean
            ),
            (gradle, "gradle --stop".into(), Selection::Clean),
            (homebrew, "brew cleanup".into(), Selection::Clean),
        ]
    );
}

#[test]
fn a_denied_fixed_path_is_a_walk_skip() {
    let fake = npm_home();
    fake.fail_on(
        "/Users/me/.npm",
        rosie::fs::Op::Lstat,
        std::io::ErrorKind::PermissionDenied,
    );

    let scan = caches(&fake);

    assert!(paths(&scan).is_empty());
    assert_eq!(skipped(&scan), [("/Users/me/.npm", WalkSkip::Denied)]);
    assert!(problems(&scan).is_empty(), "{:?}", problems(&scan));
}

// A fixed path that is a placeholder is a rule match, and a match is final: it is never
// planned or entered, with or without `--enter-placeholders`, so a fixed path inside it
// is not planned either.
#[test]
fn a_dataless_fixed_path_is_never_planned() {
    let fake = npm_home();
    add_rules(
        &fake,
        "inner.toml",
        "[rules.npm-index]\nstrategy = \"path\"\npaths = [\"~/.npm/_cacache\"]\n",
    );
    fake.make_dataless("/Users/me/.npm");

    let closed = caches(&fake);
    let opened = caches_with(
        &fake,
        Options {
            walk: rosie::config::Walk {
                enter_placeholders: true,
                ..Default::default()
            },
            ..Options::default()
        },
    );

    for scan in [closed, opened] {
        assert!(paths(&scan).is_empty());
        assert_eq!(
            skipped(&scan),
            [("/Users/me/.npm", WalkSkip::SealedPlaceholder)]
        );
    }
}

#[test]
fn fixed_paths_get_the_same_marks_as_walked_ones() {
    let fake = fake_home();
    add_rules(
        &fake,
        "marks.toml",
        "[rules.outside]\nstrategy = \"path\"\npaths = [\"/Library/Caches/tool\"]\n\n\
         [rules.rooted]\nstrategy = \"path\"\npaths = [\"~/Library/Caches/rooted\"]\n\n\
         [rules.busy]\nstrategy = \"path\"\npaths = [\"~/Library/Caches/busy\"]\n",
    );
    fake.add_file("/Library/Caches/tool/f", "x");
    fake.add_file("/Users/me/Library/Caches/rooted/f", "x");
    fake.chown("/Users/me/Library/Caches/rooted", ROOT_UID);
    fake.add_file("/Users/me/Library/Caches/busy/bin/daemon", "x");
    fake.respond_running(&[(77, "/Users/me/Library/Caches/busy/bin/daemon")]);

    let scan = caches(&fake);

    let marks: Vec<(&str, Status, RunAs)> = scan
        .plan
        .deletes()
        .iter()
        .map(|d| (d.path.to_str().expect("utf-8"), d.status.clone(), d.run_as))
        .collect();
    assert_eq!(
        marks,
        [
            ("/Library/Caches/tool", Status::OutsideRoots, RunAs::User),
            (
                "/Users/me/Library/Caches/rooted",
                Status::Clean,
                RunAs::Sudo
            ),
        ]
    );
    let refused: Vec<(PathBuf, u32)> = scan
        .refused
        .iter()
        .map(|refused| (refused.path.clone(), refused.pid))
        .collect();
    assert_eq!(
        refused,
        [(PathBuf::from("/Users/me/Library/Caches/busy"), 77)]
    );
}

#[test]
fn a_fixed_file_is_planned_as_a_file() {
    let fake = fake_home();
    add_rules(
        &fake,
        "file.toml",
        "[rules.history]\nstrategy = \"path\"\npaths = [\"~/.lesshst\"]\n",
    );
    fake.add_sized_file("/Users/me/.lesshst", 4096);

    let scan = caches(&fake);

    let [delete] = scan.plan.deletes() else {
        panic!("one delete expected: {:?}", scan.plan);
    };
    assert_eq!(delete.path, Path::new("/Users/me/.lesshst"));
    assert_eq!(delete.kind, ItemKind::File);
}

#[test]
fn a_fixed_path_below_a_file_is_absent_not_a_problem() {
    let fake = fake_home();
    add_rules(
        &fake,
        "odd.toml",
        "[rules.odd]\nstrategy = \"path\"\npaths = [\"~/.npm/cache\"]\n",
    );
    fake.add_file("/Users/me/.npm", "a file, not a folder");

    let scan = caches(&fake);

    assert!(paths(&scan).is_empty());
    assert!(scan.problems.is_empty(), "{:?}", problems(&scan));
}

#[test]
fn a_rule_listing_one_path_twice_names_it_once() {
    let fake = fake_home();
    add_rules(
        &fake,
        "twice.toml",
        "[rules.npm-cache]\nstrategy = \"path\"\npaths = [\"~/.npm\", \"/Users/me/.npm\"]\n",
    );
    fake.add_file("/Users/me/.npm/bin/npm-daemon", "x");
    fake.respond_running(&[(9, "/Users/me/.npm/bin/npm-daemon")]);

    let scan = caches(&fake);

    let [refused] = &scan.refused[..] else {
        panic!("one refusal expected: {:?}", scan.refused);
    };
    assert_eq!(refused.rules, ["rosie/npm-cache"]);
}
