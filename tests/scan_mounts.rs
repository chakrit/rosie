//! Fixture tier for other volumes in both scan modes (`docs/spec/safety.md#walk-skips`).

mod scan_fixture;

use std::path::{Path, PathBuf};

use rosie::config::Walk;
use rosie::fs::fake::{Call, FakeBackend};
use rosie::plan::{Needs, WalkSetting, WalkSkip};
use scan_fixture::*;

const NPM_CACACHE: &str =
    "[rules.npm-cacache]\nstrategy = \"path\"\npaths = [\"~/.npm/_cacache\"]\n";

fn listed(fake: &FakeBackend, path: &str) -> bool {
    fake.calls().contains(&Call::ReadDir(PathBuf::from(path)))
}

/// Whether the scan listed `root`, or listed, read, or `lstat`ed anything below it.
fn looked_inside(fake: &FakeBackend, root: &str) -> bool {
    let root = Path::new(root);
    fake.calls().iter().any(|call| match call {
        Call::ReadDir(path) | Call::ReadFile(path) => path.starts_with(root),
        Call::Lstat(path) => path.starts_with(root) && path != root,
        _ => false,
    })
}

fn mounts(enter_mounts: bool) -> Options {
    let mut options = Options::default();
    options.walk.enter_mounts = enter_mounts;
    options
}

/// [`mounts`], plus another walk flag set on top.
fn walk_flags_and_mounts(enter_mounts: bool, set: impl FnOnce(&mut Walk)) -> Options {
    let mut options = mounts(enter_mounts);
    set(&mut options.walk);
    options
}

// the tree walk

#[test]
fn other_volumes_are_not_crossed_unless_enter_mounts() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/ext/app");
    fake.mount("/Users/me/code/ext", 7);

    let closed = tree_with(&fake, CODE, mounts(false));
    let crossed = tree_with(&fake, CODE, mounts(true));

    assert!(paths(&closed).is_empty());
    assert_eq!(
        skipped(&closed),
        [("/Users/me/code/ext", WalkSkip::Closed(Needs::MOUNTS))]
    );
    assert_eq!(paths(&crossed), ["/Users/me/code/ext/app/node_modules"]);
    assert!(skipped(&crossed).is_empty());
}

/// Without `enter_mounts` the walk cannot read inside the mount root to detect a match,
/// so the skip is one the flag could open; with it, the match is known and final.
#[test]
fn a_matched_mount_root_is_a_logged_skip_and_is_not_descended() {
    for (enter_mounts, skip) in [
        (false, WalkSkip::Closed(Needs::MOUNTS)),
        (true, WalkSkip::SealedMount),
    ] {
        let fake = node_home();
        node_project(&fake, "/Users/me/code/app");
        node_project(&fake, "/Users/me/code/app/node_modules/pkg");
        fake.mount("/Users/me/code/app/node_modules", 7);

        let scan = tree_with(&fake, CODE, mounts(enter_mounts));

        assert!(paths(&scan).is_empty(), "enter_mounts = {enter_mounts}");
        assert_eq!(
            skipped(&scan),
            [("/Users/me/code/app/node_modules", skip)],
            "enter_mounts = {enter_mounts}"
        );
        assert!(!listed(&fake, "/Users/me/code/app/node_modules"));
    }
}

#[test]
fn rule_detection_never_reads_inside_a_mount_root_the_walk_may_not_enter() {
    let fake = fake_home();
    add_rules(
        &fake,
        "inside.toml",
        "[rules.npm-inside]\nstrategy = \"marker\"\ntarget = \"node_modules\"\n\
         inside = [\".package-lock.json\"]\n\
         [rules.dist-lua]\nstrategy = \"lua\"\ntarget = \"dist\"\n\
         expr = \"read_json(path .. '/package.json') ~= nil and #glob(path .. '/*') > 0\"\n",
    );
    fake.add_file("/Users/me/code/app/node_modules/.package-lock.json", "{}");
    fake.add_file("/Users/me/code/web/dist/package.json", "{}");
    fake.mount("/Users/me/code/app/node_modules", 7);
    fake.mount("/Users/me/code/web/dist", 8);

    let scan = tree_with(&fake, CODE, mounts(false));

    assert!(paths(&scan).is_empty());
    assert_eq!(
        skipped(&scan),
        [
            (
                "/Users/me/code/app/node_modules",
                WalkSkip::Closed(Needs::MOUNTS)
            ),
            ("/Users/me/code/web/dist", WalkSkip::Closed(Needs::MOUNTS)),
        ]
    );
    assert!(!looked_inside(&fake, "/Users/me/code/app/node_modules"));
    assert!(!looked_inside(&fake, "/Users/me/code/web/dist"));
}

#[test]
fn a_mounted_bundle_is_not_entered_without_enter_bundles() {
    // Without `enter_mounts` neither flag alone crosses it, so its skip needs both; with
    // it, only `enter_bundles` is missing.
    let mounted_bundle = Needs::MOUNTS.and(Some(WalkSetting::Bundles));
    for (enter_mounts, needs) in [(false, mounted_bundle), (true, Needs::BUNDLES)] {
        let fake = node_home();
        node_project(&fake, "/Users/me/code/Tool.app/Contents/app");
        fake.mount("/Users/me/code/Tool.app", 8);

        let scan = tree_with(&fake, CODE, mounts(enter_mounts));

        assert!(paths(&scan).is_empty(), "enter_mounts = {enter_mounts}");
        assert_eq!(
            skipped(&scan),
            [("/Users/me/code/Tool.app", WalkSkip::Closed(needs))],
            "enter_mounts = {enter_mounts}"
        );
        assert!(!listed(&fake, "/Users/me/code/Tool.app"));
    }

    let both = walk_flags_and_mounts(true, |walk| walk.enter_bundles = true);
    let fake = node_home();
    node_project(&fake, "/Users/me/code/Tool.app/Contents/app");
    fake.mount("/Users/me/code/Tool.app", 8);
    let entered = tree_with(&fake, CODE, both);
    assert_eq!(
        paths(&entered),
        ["/Users/me/code/Tool.app/Contents/app/node_modules"]
    );
    assert!(entered.skipped.is_empty());
}

#[test]
fn a_mounted_placeholder_is_not_opened_without_enter_placeholders() {
    // Its skip carries every flag still needed to open it.
    let mounted_placeholder = Needs::PLACEHOLDERS.and(Some(WalkSetting::Mounts));
    for (enter_mounts, needs) in [(false, mounted_placeholder), (true, Needs::PLACEHOLDERS)] {
        let fake = node_home();
        node_project(&fake, "/Users/me/code/cloud/app");
        fake.mount("/Users/me/code/cloud", 9);
        fake.make_dataless("/Users/me/code/cloud");

        let scan = tree_with(&fake, CODE, mounts(enter_mounts));

        assert!(paths(&scan).is_empty(), "enter_mounts = {enter_mounts}");
        assert_eq!(
            skipped(&scan),
            [("/Users/me/code/cloud", WalkSkip::Closed(needs))],
            "enter_mounts = {enter_mounts}"
        );
        assert!(!listed(&fake, "/Users/me/code/cloud"));
    }
}

#[test]
fn a_fixed_path_that_is_a_mount_root_is_never_planned_by_the_walk() {
    for enter_mounts in [false, true] {
        let fake = fake_home();
        add_rules(
            &fake,
            "npm.toml",
            "[rules.npm-cache]\nstrategy = \"path\"\npaths = [\"~/.npm\"]\n",
        );
        fake.add_file("/Users/me/.npm/_cacache/x", "x");
        fake.mount("/Users/me/.npm", 5);

        let scan = tree_with(&fake, HOME, mounts(enter_mounts));

        assert!(paths(&scan).is_empty(), "enter_mounts = {enter_mounts}");
        assert_eq!(
            skipped(&scan),
            [("/Users/me/.npm", WalkSkip::SealedMount)],
            "enter_mounts = {enter_mounts}"
        );
    }
}

#[test]
fn the_walk_reaches_a_fixed_path_inside_a_mounted_folder_only_with_enter_mounts() {
    let scenario = || {
        let fake = fake_home();
        add_rules(&fake, "npm.toml", NPM_CACACHE);
        fake.add_file("/Users/me/.npm/_cacache/x", "x");
        fake.mount("/Users/me/.npm", 5);
        fake
    };

    let closed = tree_with(&scenario(), HOME, mounts(false));
    let crossed = tree_with(&scenario(), HOME, mounts(true));

    assert!(paths(&closed).is_empty());
    assert_eq!(
        skipped(&closed),
        [("/Users/me/.npm", WalkSkip::Closed(Needs::MOUNTS))]
    );
    assert_eq!(paths(&crossed), ["/Users/me/.npm/_cacache"]);
}

// fixed rule paths in `caches`

#[test]
fn a_fixed_path_that_is_a_mount_root_is_never_planned() {
    for enter_mounts in [false, true] {
        let fake = fake_home();
        add_rules(
            &fake,
            "npm.toml",
            "[rules.npm-cache]\nstrategy = \"path\"\npaths = [\"~/.npm\"]\n",
        );
        fake.add_file("/Users/me/.npm/_cacache/x", "x");
        fake.mount("/Users/me/.npm", 5);

        let scan = caches_with(&fake, mounts(enter_mounts));

        assert!(paths(&scan).is_empty(), "enter_mounts = {enter_mounts}");
        assert_eq!(
            skipped(&scan),
            [("/Users/me/.npm", WalkSkip::SealedMount)],
            "enter_mounts = {enter_mounts}"
        );
    }
}

#[test]
fn a_fixed_path_inside_a_mounted_folder_is_planned_only_with_enter_mounts() {
    let scenario = || {
        let fake = fake_home();
        add_rules(&fake, "npm.toml", NPM_CACACHE);
        fake.add_file("/Users/me/.npm/_cacache/x", "x");
        fake.mount("/Users/me/.npm", 5);
        fake
    };

    let closed = caches_with(&scenario(), mounts(false));
    let crossed = caches_with(&scenario(), mounts(true));

    assert!(paths(&closed).is_empty());
    assert_eq!(
        skipped(&closed),
        [("/Users/me/.npm/_cacache", WalkSkip::Closed(Needs::MOUNTS))]
    );
    assert_eq!(paths(&crossed), ["/Users/me/.npm/_cacache"]);
    assert!(skipped(&crossed).is_empty());
}

#[test]
fn a_mount_anywhere_between_the_anchor_and_a_fixed_path_is_a_crossing() {
    let scenario = || {
        let fake = fake_home();
        add_rules(
            &fake,
            "deep.toml",
            "[rules.deep]\nstrategy = \"path\"\npaths = [\"~/vol/a/b/cache\", \"/Library/Caches/Tool\"]\n",
        );
        fake.add_file("/Users/me/vol/a/b/cache/x", "x");
        fake.add_file("/Library/Caches/Tool/x", "x");
        fake.mount("/Users/me/vol", 6);
        fake.mount("/Library", 7);
        fake
    };

    let closed = caches_with(&scenario(), mounts(false));
    let crossed = caches_with(&scenario(), mounts(true));

    assert!(paths(&closed).is_empty());
    assert_eq!(
        skipped(&closed),
        [
            ("/Library/Caches/Tool", WalkSkip::Closed(Needs::MOUNTS)),
            ("/Users/me/vol/a/b/cache", WalkSkip::Closed(Needs::MOUNTS)),
        ]
    );
    assert_eq!(
        paths(&crossed),
        ["/Library/Caches/Tool", "/Users/me/vol/a/b/cache"]
    );
}

#[test]
fn a_home_on_its_own_volume_plans_its_fixed_paths_however_home_is_spelled() {
    for home in [HOME, "/Users/me/../me"] {
        let fake = fake_home();
        add_rules(&fake, "npm.toml", NPM_CACACHE);
        fake.add_file("/Users/me/.npm/_cacache/x", "x");
        fake.mount(HOME, 3);
        let options = Options {
            home: PathBuf::from(home),
            ..Options::default()
        };

        let scan = caches_with(&fake, options);

        assert_eq!(paths(&scan), ["/Users/me/.npm/_cacache"], "home = {home}");
        assert!(skipped(&scan).is_empty(), "home = {home}");
    }
}
