//! Pulling, replacing, removing, and aging rule packs (`docs/spec/rules.md#packs`),
//! against the in-memory fake and a canned network. What an interrupted pull or remove
//! leaves behind is covered in `pack_recovery.rs`.

mod pack_fixture;
mod tarball;

use std::path::Path;
use std::time::{Duration, SystemTime};

use pack_fixture::{
    Canned, DATA, NODE_RULE, ROSIE_PACK, ROSIE_URL, at, contents, ctx, fake_home, installed,
    listing, source, url_of,
};
use rosie::packs::{self, Error, FirstRun, Installed, MONTH, RemovePack, Source, Store};
use rosie::rules::{self, Problem};
use tarball::{TOP, Tarball};

// pull

#[test]
fn pull_installs_only_the_rules_folder_into_the_pack_folder() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE), ("rust.toml", "")])
        .file(&format!("{TOP}/README.md"), b"# rosie")
        .gzip();
    let network = Canned::serving(ROSIE_URL, tarball);

    let pulled =
        packs::pull(&store, &network, &Source::default(), at(1_000)).expect("pull succeeds");

    assert_eq!(pulled.source, Source::default());
    assert_eq!(network.requested(), vec![ROSIE_URL]);
    assert_eq!(
        contents(&fake, format!("{ROSIE_PACK}/node.toml")),
        NODE_RULE
    );
    assert!(fake.exists(format!("{ROSIE_PACK}/rust.toml")));
    assert!(!fake.exists(format!("{ROSIE_PACK}/README.md")));
    assert!(!fake.exists(format!("{ROSIE_PACK}/config.toml")));
    assert_eq!(
        installed(&fake),
        vec![Installed {
            source: Source::default(),
            pulled_at: at(1_000),
        }]
    );
}

#[test]
fn pull_replaces_a_pack_wholesale() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let first = Tarball::pack(&[("a.toml", "# old a"), ("b.toml", "# old b")]).gzip();
    let second = Tarball::pack(&[("b.toml", "# new b"), ("c.toml", "# new c")]).gzip();

    packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, first),
        &Source::default(),
        at(1),
    )
    .expect("first pull");
    packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, second),
        &Source::default(),
        at(2),
    )
    .expect("second pull");

    assert!(!fake.exists(format!("{ROSIE_PACK}/a.toml")));
    assert_eq!(contents(&fake, format!("{ROSIE_PACK}/b.toml")), "# new b");
    assert_eq!(contents(&fake, format!("{ROSIE_PACK}/c.toml")), "# new c");
    assert_eq!(
        listing(&fake, &format!("{DATA}/packs/chakrit")),
        vec!["rosie"]
    );
    assert_eq!(installed(&fake)[0].pulled_at, at(2));
}

#[test]
fn a_refused_tarball_leaves_the_installed_pack_and_the_home_untouched() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let good = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, good),
        &Source::default(),
        at(1),
    )
    .expect("first pull");
    let evil = Tarball::pack(&[("node.toml", "# replaced")])
        .file("/Users/me/.zshrc", b"pwned")
        .file(&format!("{TOP}/rules/../../../../.zshrc"), b"pwned")
        .gzip();

    let result = packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, evil),
        &Source::default(),
        at(2),
    );

    assert!(
        matches!(result, Err(Error::Unsafe { .. })),
        "got {result:?}"
    );
    assert!(!fake.exists("/Users/me/.zshrc"));
    assert_eq!(
        contents(&fake, format!("{ROSIE_PACK}/node.toml")),
        NODE_RULE
    );
    assert_eq!(installed(&fake)[0].pulled_at, at(1));
}

#[test]
fn pull_refuses_rule_files_the_volume_folds_into_one_name() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let good = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, good),
        &Source::default(),
        at(1),
    )
    .expect("first pull");
    let clashing = Tarball::pack(&[("ok.toml", "# first"), ("OK.toml", "# second")]).gzip();

    let result = packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, clashing),
        &Source::default(),
        at(2),
    );

    assert!(
        matches!(&result, Err(Error::Duplicate { path }) if path == Path::new("rules/OK.toml")),
        "got {result:?}"
    );
    assert_eq!(listing(&fake, ROSIE_PACK), vec![".pulled", "node.toml"]);
    assert_eq!(installed(&fake)[0].pulled_at, at(1));
}

#[test]
fn pull_refuses_a_rule_file_that_is_not_toml() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let tarball = Tarball::pack(&[("broken.toml", "[rules.x\n")]).gzip();

    let result = packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, tarball),
        &Source::default(),
        at(1),
    );

    assert!(
        matches!(result, Err(Error::InvalidRules(_))),
        "got {result:?}"
    );
    assert!(!fake.exists(ROSIE_PACK));
}

#[test]
fn pull_refuses_a_pack_whose_rule_fails_to_load_and_keeps_the_installed_one() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let good = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, good),
        &Source::default(),
        at(1),
    )
    .expect("first pull");
    // Valid TOML, but the `name` strategy takes no `marker` field: a rule the parser
    // refuses, unlike the malformed-TOML case above.
    let bad_rule = Tarball::pack(&[(
        "bad.toml",
        "[rules.x]\nstrategy = \"name\"\ntarget = \"x\"\nmarker = [\"a\"]",
    )])
    .gzip();

    let result = packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, bad_rule),
        &Source::default(),
        at(2),
    );

    assert!(
        matches!(result, Err(Error::InvalidRules(_))),
        "got {result:?}"
    );
    assert_eq!(
        contents(&fake, format!("{ROSIE_PACK}/node.toml")),
        NODE_RULE
    );
    assert_eq!(installed(&fake)[0].pulled_at, at(1));
}

#[test]
fn pull_refuses_a_pack_name_already_installed_from_another_owner() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let network =
        Canned::serving(ROSIE_URL, tarball.clone()).and(&url_of("someone/rosie"), tarball);
    packs::pull(&store, &network, &Source::default(), at(1)).expect("first pull");

    let result = packs::pull(&store, &network, &source("someone/rosie"), at(2));

    assert!(
        matches!(&result, Err(Error::PackNameTaken { pack, installed }) if pack == "rosie" && *installed == Source::default()),
        "got {result:?}"
    );
    assert!(!fake.exists(format!("{DATA}/packs/someone")));
}

/// GitHub treats owner and repository names without regard to case, so pack sources and
/// names compare ignoring ASCII case (`docs/spec/rules.md#packs`).
#[test]
fn pull_replaces_a_pack_requested_in_another_case_under_its_installed_spelling() {
    for requested in ["chakrit/Rosie", "Chakrit/ROSIE"] {
        let fake = fake_home();
        let ctx = ctx(&fake);
        let store = Store::new(&ctx, Path::new(DATA));
        let first = Tarball::pack(&[("old.toml", "# old")]).gzip();
        let second = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
        packs::pull(
            &store,
            &Canned::serving(ROSIE_URL, first),
            &Source::default(),
            at(1),
        )
        .expect("first pull");

        let network = Canned::serving(ROSIE_URL, second.clone()).and(&url_of(requested), second);
        let pulled = packs::pull(&store, &network, &source(requested), at(2))
            .unwrap_or_else(|error| panic!("{requested}: {error:?}"));

        assert_eq!(pulled.source, Source::default(), "{requested}");
        assert_eq!(listing(&fake, &format!("{DATA}/packs")), vec!["chakrit"]);
        assert_eq!(
            listing(&fake, &format!("{DATA}/packs/chakrit")),
            vec!["rosie"]
        );
        assert!(
            !fake.exists(format!("{ROSIE_PACK}/old.toml")),
            "{requested}"
        );
        assert_eq!(
            installed(&fake),
            vec![Installed {
                source: Source::default(),
                pulled_at: at(2),
            }]
        );
    }
}

#[test]
fn pull_refuses_a_pack_name_installed_from_another_owner_in_another_case() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let network =
        Canned::serving(ROSIE_URL, tarball.clone()).and(&url_of("someone/Rosie"), tarball);
    packs::pull(&store, &network, &Source::default(), at(1)).expect("first pull");

    let result = packs::pull(&store, &network, &source("someone/Rosie"), at(2));

    assert!(
        matches!(&result, Err(Error::PackNameTaken { installed, .. }) if *installed == Source::default()),
        "got {result:?}"
    );
    assert!(!fake.exists(format!("{DATA}/packs/someone")));
}

/// Pulls the default pack with `node.toml`, then pulls it again as `files`, which must be
/// refused as invalid rules while the first pack stays installed. Returns the refusal's
/// inner rule error.
fn refused_pull_keeps_the_installed_pack(files: &[(&str, &str)]) -> rules::Error {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let good = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, good),
        &Source::default(),
        at(1),
    )
    .expect("first pull");

    let result = packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, Tarball::pack(files).gzip()),
        &Source::default(),
        at(2),
    );

    assert_eq!(listing(&fake, ROSIE_PACK), vec![".pulled", "node.toml"]);
    assert_eq!(
        contents(&fake, format!("{ROSIE_PACK}/node.toml")),
        NODE_RULE
    );
    assert_eq!(installed(&fake)[0].pulled_at, at(1));
    match result {
        Err(Error::InvalidRules(inner)) => *inner,
        other => panic!("expected the pack's rules to be refused, got {other:?}"),
    }
}

#[test]
fn pull_refuses_a_rule_key_that_is_not_a_valid_name() {
    let error = refused_pull_keeps_the_installed_pack(&[(
        "bad.toml",
        "[rules.\"a.b\"]\nstrategy = \"name\"\ntarget = \"x\"",
    )]);

    assert!(
        matches!(&error, rules::Error::Rule { rule, problem: Problem::Name(_), .. } if rule == "a.b"),
        "got {error}"
    );
}

#[test]
fn pull_refuses_an_override_by_qualified_name_in_a_pack() {
    let error = refused_pull_keeps_the_installed_pack(&[(
        "bad.toml",
        "[rules.\"other/x\"]\nstrategy = \"name\"\ntarget = \"x\"",
    )]);

    assert!(
        matches!(
            &error,
            rules::Error::Rule {
                problem: Problem::OverrideInPack,
                ..
            }
        ),
        "got {error}"
    );
}

#[test]
fn pull_refuses_a_rule_defined_in_two_files_of_the_pack() {
    let rule = "[rules.x]\nstrategy = \"name\"\ntarget = \"x\"";

    let error = refused_pull_keeps_the_installed_pack(&[("a.toml", rule), ("b.toml", rule)]);

    assert!(
        matches!(&error, rules::Error::DuplicateRule { rule, .. } if rule.to_string() == "rosie/x"),
        "got {error}"
    );
}

// seeding a config

#[test]
fn seeding_without_packs_installs_the_default_pack_from_the_one_download() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let network = Canned::serving(ROSIE_URL, tarball);

    let seed = packs::seed_config(&store, &network, at(5)).expect("seeded");

    assert_eq!(seed.config, "roots = []\n");
    assert!(
        matches!(&seed.first_run, FirstRun::Pulled(pulled) if pulled.source == Source::default()),
        "got {:?}",
        seed.first_run
    );
    assert_eq!(
        contents(&fake, format!("{ROSIE_PACK}/node.toml")),
        NODE_RULE
    );
    assert_eq!(network.requested(), vec![ROSIE_URL]);
}

#[test]
fn seeding_with_a_pack_installed_leaves_the_packs_alone() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let old = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, old),
        &Source::default(),
        at(1),
    )
    .expect("pull");
    let newer = Tarball::pack(&[("other.toml", "")]).gzip();

    let seed =
        packs::seed_config(&store, &Canned::serving(ROSIE_URL, newer), at(5)).expect("seeded");

    assert_eq!(seed.config, "roots = []\n");
    assert!(
        matches!(seed.first_run, FirstRun::Ready),
        "got {:?}",
        seed.first_run
    );
    assert!(!fake.exists(format!("{ROSIE_PACK}/other.toml")));
    assert_eq!(
        installed(&fake),
        vec![Installed {
            source: Source::default(),
            pulled_at: at(1),
        }]
    );
}

// remove

#[test]
fn removes_any_pack_including_rosie() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, tarball),
        &Source::default(),
        at(1),
    )
    .expect("pull");

    let removed = RemovePack {
        pack: "rosie".to_owned(),
    }
    .run(&store)
    .expect("remove rosie");

    assert_eq!(removed, Source::default());
    assert!(!fake.exists(ROSIE_PACK));
    assert_eq!(installed(&fake), vec![]);
}

#[test]
fn removes_a_pack_named_in_another_case() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, tarball),
        &Source::default(),
        at(1),
    )
    .expect("pull");

    let removed = RemovePack {
        pack: "Rosie".to_owned(),
    }
    .run(&store)
    .expect("remove Rosie");

    assert_eq!(removed, Source::default());
    assert_eq!(installed(&fake), vec![]);
}

#[test]
fn removing_a_pack_that_is_not_installed_fails() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));

    let result = RemovePack {
        pack: "rosie".to_owned(),
    }
    .run(&store);

    assert!(
        matches!(&result, Err(Error::NotInstalled { pack }) if pack == "rosie"),
        "got {result:?}"
    );
}

// first run

#[test]
fn first_run_without_packs_pulls_the_default_source() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let network = Canned::serving(ROSIE_URL, tarball);

    let outcome = packs::first_run(&store, &network, at(5)).expect("first run pull");

    assert!(
        matches!(&outcome, FirstRun::Pulled(pulled) if pulled.source == Source::default()),
        "got {outcome:?}"
    );
    assert_eq!(
        contents(&fake, format!("{ROSIE_PACK}/node.toml")),
        NODE_RULE
    );
}

#[test]
fn first_run_with_a_pack_does_not_touch_the_network() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let tarball = Tarball::pack(&[("js.toml", NODE_RULE)]).gzip();
    packs::pull(
        &store,
        &Canned::serving(&url_of("someone/extra"), tarball),
        &source("someone/extra"),
        at(1),
    )
    .expect("pull");
    let network = Canned::offline();

    let outcome = packs::first_run(&store, &network, at(5)).expect("ready");

    assert!(matches!(outcome, FirstRun::Ready), "got {outcome:?}");
    assert_eq!(network.requested(), Vec::<String>::new());
}

#[test]
fn first_run_offline_stops_with_the_reason() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));

    let result = packs::first_run(&store, &Canned::offline(), at(5));

    let Err(error) = result else {
        panic!("expected the offline pull to fail, got {result:?}");
    };
    assert!(matches!(&error, Error::Download { url, .. } if url == ROSIE_URL));
    assert!(
        error.to_string().contains("the network is unreachable"),
        "the reason is named: {error}"
    );
    assert_eq!(installed(&fake), vec![]);
}

#[test]
fn removing_the_last_pack_makes_the_next_run_pull_rosie_again() {
    let fake = fake_home();
    let ctx = ctx(&fake);
    let store = Store::new(&ctx, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let network = Canned::serving(ROSIE_URL, tarball);
    packs::pull(&store, &network, &Source::default(), at(1)).expect("pull");
    RemovePack {
        pack: "rosie".to_owned(),
    }
    .run(&store)
    .expect("remove");

    let outcome = packs::first_run(&store, &network, at(5)).expect("pull again");

    assert!(matches!(outcome, FirstRun::Pulled(_)), "got {outcome:?}");
}

// age

fn pack_at(pulled_at: SystemTime) -> Installed {
    Installed {
        source: Source::default(),
        pulled_at,
    }
}

#[test]
fn warns_once_the_rules_are_six_months_old() {
    let pulled_at = at(1_000_000);
    let six_months = pulled_at + MONTH * 6;

    let just_before =
        packs::age_warning(&[pack_at(pulled_at)], six_months - Duration::from_secs(1));
    let at_six = packs::age_warning(&[pack_at(pulled_at)], six_months);

    assert_eq!(just_before, None);
    assert_eq!(
        at_six.map(|warning| warning.to_string()),
        Some("Your rules are 6 months old, consider rosie rules pull to update".to_owned())
    );
}

#[test]
fn ages_the_rules_by_the_oldest_pack() {
    let now = at(100_000_000);
    let packs = [pack_at(now - MONTH * 2), pack_at(now - MONTH * 13)];

    let warning = packs::age_warning(&packs, now);

    assert_eq!(
        warning.map(|warning| warning.to_string()),
        Some("Your rules are 13 months old, consider rosie rules pull to update".to_owned())
    );
}

#[test]
fn a_pull_time_in_the_future_is_not_stale() {
    let now = at(1_000);

    let warning = packs::age_warning(&[pack_at(now + MONTH * 12)], now);

    assert_eq!(warning, None);
}
