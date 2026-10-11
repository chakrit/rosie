//! Fixture tier for `enter_links`: paths rosie is given through symlinks, and the walk
//! following links (`docs/spec/safety.md#enter_links`).

mod scan_fixture;

use std::path::{Path, PathBuf};

use rosie::config::Walk;
use rosie::fs::fake::{Call, FakeBackend};
use rosie::fs::{self, EnterLinks};
use rosie::scan::{Error, Needs, NoProgress, WalkSkip};
use scan_fixture::*;

fn listed(fake: &FakeBackend, path: &str) -> bool {
    fake.calls().contains(&Call::ReadDir(PathBuf::from(path)))
}

// `enter_links` (`docs/spec/safety.md#enter_links`)

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
fn a_fixed_path_through_a_symlink_is_matched_at_its_real_location_when_links_are_allowed() {
    let fake = fake_home();
    add_rules(
        &fake,
        "caches.toml",
        "[rules.tool-cache]\nstrategy = \"path\"\npaths = [\"~/Library/Caches/tool\"]\n",
    );
    fake.add_file("/Users/me/real-caches/tool/f", "x");
    fake.add_symlink("/Users/me/Library/Caches", "/Users/me/real-caches");

    let scan = tree_with(&fake, HOME, links(EnterLinks::Roots));

    assert_eq!(paths(&scan), ["/Users/me/real-caches/tool"]);
    assert!(scan.problems.is_empty(), "{:?}", problems(&scan));
}

#[test]
fn a_scan_folder_through_a_symlink_is_walked_at_its_real_location_when_links_are_allowed() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/app");
    fake.add_symlink("/Users/me/work", "/Users/me/code");

    let refused = scanner(&fake, Options::default()).tree(Path::new("/Users/me/work"), &NoProgress);
    let scan = tree_with(&fake, "/Users/me/work", links(EnterLinks::Roots));

    assert!(
        matches!(refused, Err(Error::Fs(fs::Error::Symlink { .. }))),
        "{refused:?}"
    );
    assert_eq!(paths(&scan), ["/Users/me/code/app/node_modules"]);
}

// `enter_links = "all"`: the walk follows links

/// A link inside the scan folder to a node project elsewhere in the roots, and one to a
/// `node_modules` folder, as `symlinks_are_never_entered_or_matched` sets them up.
fn linked_projects() -> FakeBackend {
    let fake = node_home();
    fake.add_file("/Users/me/code/app/package.json", "{}");
    fake.add_file("/Users/me/elsewhere/nm/index.js", "x");
    fake.add_symlink("/Users/me/code/app/node_modules", "/Users/me/elsewhere/nm");
    node_project(&fake, "/Users/me/other/proj");
    fake.add_symlink("/Users/me/code/linked", "/Users/me/other");
    fake
}

#[test]
fn the_walk_follows_a_link_into_the_roots_and_plans_at_the_real_location() {
    let fake = linked_projects();

    let roots_only = tree_with(&fake, CODE, links(EnterLinks::Roots));
    let all = tree_with(&fake, CODE, links(EnterLinks::All));

    assert!(paths(&roots_only).is_empty(), "{:?}", paths(&roots_only));
    assert_eq!(paths(&all), ["/Users/me/other/proj/node_modules"]);
    assert!(listed(&fake, "/Users/me/other/proj"));
}

/// The link itself is never a target: its target is looked at where it is, so a link
/// named `node_modules` to a folder named `nm` matches nothing.
#[test]
fn a_followed_link_is_matched_by_its_target_not_its_own_name() {
    let fake = linked_projects();

    let scan = tree_with(&fake, CODE, links(EnterLinks::All));

    assert!(!paths(&scan).contains(&"/Users/me/elsewhere/nm"));
    assert!(!paths(&scan).contains(&"/Users/me/code/app/node_modules"));
}

#[test]
fn a_link_whose_target_is_outside_the_roots_is_not_followed() {
    let fake = linked_projects();
    let options = Options {
        roots: vec![PathBuf::from(CODE)],
        ..links(EnterLinks::All)
    };

    let scan = tree_with(&fake, CODE, options);

    assert!(paths(&scan).is_empty(), "{:?}", paths(&scan));
    assert!(!listed(&fake, "/Users/me/other"));
    assert!(
        skipped(&scan).contains(&("/Users/me/other", WalkSkip::LinkOutsideRoots)),
        "{:?}",
        skipped(&scan)
    );
}

#[test]
fn a_link_onto_another_volume_is_followed_only_with_enter_mounts() {
    let fake = linked_projects();
    fake.mount("/Users/me/other", 7);
    let mut both = links(EnterLinks::All);
    both.walk.enter_mounts = true;

    let closed = tree_with(&fake, CODE, links(EnterLinks::All));
    let opened = tree_with(&fake, CODE, both);

    assert!(paths(&closed).is_empty(), "{:?}", paths(&closed));
    assert!(
        skipped(&closed).contains(&("/Users/me/code/linked", WalkSkip::Closed(Needs::MOUNTS))),
        "{:?}",
        skipped(&closed)
    );
    assert_eq!(paths(&opened), ["/Users/me/other/proj/node_modules"]);
}

#[test]
fn links_back_into_walked_folders_and_link_cycles_end() {
    let fake = node_home();
    node_project(&fake, "/Users/me/code/app");
    fake.add_symlink("/Users/me/code/app/up", "/Users/me/code");
    fake.add_symlink("/Users/me/code/a", "/Users/me/code/b");
    fake.add_symlink("/Users/me/code/b", "/Users/me/code/a");
    fake.add_symlink("/Users/me/code/gone", "/Users/me/nothing");

    let scan = tree_with(&fake, CODE, links(EnterLinks::All));

    assert_eq!(paths(&scan), ["/Users/me/code/app/node_modules"]);
    let skips = skipped(&scan);
    assert!(
        skips.contains(&("/Users/me/code/app/up", WalkSkip::LinkWalked)),
        "{skips:?}"
    );
    assert!(
        skips.contains(&("/Users/me/code/a", WalkSkip::LinkLoop)),
        "{skips:?}"
    );
    assert!(
        skips.contains(&("/Users/me/code/gone", WalkSkip::LinkDangling)),
        "{skips:?}"
    );
}

#[test]
fn two_links_to_one_target_plan_it_once() {
    let fake = node_home();
    node_project(&fake, "/Users/me/other/proj");
    fake.add_symlink("/Users/me/code/one", "/Users/me/other");
    fake.add_symlink("/Users/me/code/two", "/Users/me/other");

    let scan = tree_with(&fake, CODE, links(EnterLinks::All));

    assert_eq!(paths(&scan), ["/Users/me/other/proj/node_modules"]);
}
