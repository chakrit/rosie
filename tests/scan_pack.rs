//! Fixture tier with the repository's own rule pack: a realistic workspace of node,
//! cargo, and python projects, checking what the rules match and what they leave alone
//! (`docs/spec/testing.md#tiers`, `docs/spec/overview.md#shape`).

mod scan_fixture;

use rosie::fs::fake::FakeBackend;
use scan_fixture::*;

/// Every rule file of the repository's `rules/` pack.
const PACK_FILES: [(&str, &str); 15] = [
    ("cocoapods.toml", include_str!("../rules/cocoapods.toml")),
    ("docker.toml", include_str!("../rules/docker.toml")),
    ("flutter.toml", include_str!("../rules/flutter.toml")),
    ("go.toml", include_str!("../rules/go.toml")),
    ("homebrew.toml", include_str!("../rules/homebrew.toml")),
    ("ide.toml", include_str!("../rules/ide.toml")),
    (
        "js-frameworks.toml",
        include_str!("../rules/js-frameworks.toml"),
    ),
    ("jvm.toml", include_str!("../rules/jvm.toml")),
    ("logs.toml", include_str!("../rules/logs.toml")),
    ("node.toml", include_str!("../rules/node.toml")),
    ("python.toml", include_str!("../rules/python.toml")),
    ("ruby.toml", include_str!("../rules/ruby.toml")),
    ("rust.toml", include_str!("../rules/rust.toml")),
    ("swift.toml", include_str!("../rules/swift.toml")),
    ("terraform.toml", include_str!("../rules/terraform.toml")),
];

fn workspace() -> FakeBackend {
    let fake = fake_home();
    for (file, text) in PACK_FILES {
        add_rules(&fake, file, text);
    }

    // node: a project with nested packages, and a stray folder without a manifest.
    fake.add_file("/Users/me/code/web/package.json", "{}");
    fake.add_file("/Users/me/code/web/src/index.js", "x");
    fake.add_file("/Users/me/code/web/node_modules/react/package.json", "{}");
    fake.add_file(
        "/Users/me/code/web/node_modules/react/node_modules/x/i.js",
        "x",
    );
    fake.add_file("/Users/me/code/scratch/node_modules/left-over.js", "x");

    // cargo: a crate's target, and a `target` folder that is not cargo's.
    fake.add_file("/Users/me/code/crate/Cargo.toml", "[package]");
    fake.add_file("/Users/me/code/crate/src/main.rs", "fn main() {}");
    fake.add_file("/Users/me/code/crate/target/debug/crate", "x");
    fake.add_file("/Users/me/code/notes/target/keep.txt", "x");

    // python: a venv, byte-code caches inside and outside it, and a venv-named folder
    // that is not a virtual environment.
    fake.add_file("/Users/me/code/py/pyproject.toml", "[project]");
    fake.add_file("/Users/me/code/py/.venv/pyvenv.cfg", "home = /usr/bin");
    fake.add_file("/Users/me/code/py/.venv/lib/__pycache__/site.pyc", "x");
    fake.add_file("/Users/me/code/py/pkg/__pycache__/mod.pyc", "x");
    fake.add_file("/Users/me/code/py/pkg/mod.py", "x");
    fake.add_file("/Users/me/code/loose/requirements.txt", "");
    fake.add_file("/Users/me/code/loose/venv/bin/python", "x");

    // caches the pack names by fixed path.
    fake.add_file("/Users/me/.npm/_cacache/index", "x");
    fake.add_file("/Users/me/.cargo/registry/cache/crate.crate", "x");
    fake.add_file("/Users/me/Library/Caches/pip/http/x", "x");
    fake
}

#[test]
fn the_pack_matches_project_artifacts_and_leaves_sources_alone() {
    let fake = workspace();

    let scan = tree(&fake, "/Users/me/code");

    assert_eq!(
        entries(&scan)
            .into_iter()
            .map(|(path, rules, _)| (path, rules))
            .collect::<Vec<_>>(),
        [
            ("/Users/me/code/crate/target", vec!["rosie/cargo-target"]),
            ("/Users/me/code/py/.venv", vec!["rosie/python-venv"]),
            ("/Users/me/code/py/pkg/__pycache__", vec!["rosie/pycache"]),
            (
                "/Users/me/code/web/node_modules",
                vec!["rosie/node-modules"]
            ),
        ]
    );
    assert!(scan.problems.is_empty(), "{:?}", problems(&scan));
}

#[test]
fn a_home_scan_adds_the_packs_fixed_paths_under_it() {
    let fake = workspace();

    let scan = tree(&fake, HOME);

    let planned = paths(&scan);
    for cache in [
        "/Users/me/.cargo/registry/cache",
        "/Users/me/.npm",
        "/Users/me/Library/Caches/pip",
    ] {
        assert!(planned.contains(&cache), "{cache} missing from {planned:?}");
    }
    assert!(planned.contains(&"/Users/me/code/web/node_modules"));
    assert!(scan.plan.tools().is_empty());
    assert!(scan.problems.is_empty(), "{:?}", problems(&scan));
}

#[test]
fn a_caches_scan_takes_the_packs_fixed_paths_and_tools() {
    let fake = workspace();

    let scan = caches(&fake);

    assert_eq!(
        paths(&scan),
        [
            "/Users/me/.cargo/registry/cache",
            "/Users/me/.npm",
            "/Users/me/Library/Caches/pip",
        ]
    );
    let commands: Vec<String> = scan
        .plan
        .tools()
        .iter()
        .map(|tool| tool.command.words().collect::<Vec<_>>().join(" "))
        .collect();
    assert!(
        commands.contains(&"docker system prune --force".to_owned()),
        "{commands:?}"
    );
}
