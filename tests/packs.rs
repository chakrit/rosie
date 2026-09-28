//! Pulling, replacing, removing, and aging rule packs (`docs/spec/rules.md#packs`),
//! against the in-memory fake and a canned network. What an interrupted pull or remove
//! leaves behind is covered in `pack_recovery.rs`.

mod pack_fixture;
mod tarball;

use std::path::Path;
use std::time::{Duration, SystemTime};

use pack_fixture::{
    Canned, DATA, NODE_RULE, ROSIE_PACK, ROSIE_URL, at, contents, fake_home, gate, listing, source,
    url_of,
};
use rosie::packs::{self, Error, FirstRun, Installed, MONTH, RemovePack, Source, Store};
use tarball::{TOP, Tarball};

// pull

#[test]
fn pull_installs_only_the_rules_folder_into_the_pack_folder() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
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
        store.installed().expect("list packs"),
        vec![Installed {
            source: Source::default(),
            pulled_at: at(1_000),
        }]
    );
}

#[test]
fn pull_replaces_a_pack_wholesale() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
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
    assert_eq!(store.installed().expect("list packs")[0].pulled_at, at(2));
}

#[test]
fn a_refused_tarball_leaves_the_installed_pack_and_the_home_untouched() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
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
    assert_eq!(store.installed().expect("list packs")[0].pulled_at, at(1));
}

#[test]
fn pull_refuses_rule_files_the_volume_folds_into_one_name() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
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
    assert_eq!(store.installed().expect("list packs")[0].pulled_at, at(1));
}

#[test]
fn pull_refuses_a_rule_file_that_is_not_toml() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
    let tarball = Tarball::pack(&[("broken.toml", "[rules.x\n")]).gzip();

    let result = packs::pull(
        &store,
        &Canned::serving(ROSIE_URL, tarball),
        &Source::default(),
        at(1),
    );

    assert!(
        matches!(result, Err(Error::RuleFile { .. })),
        "got {result:?}"
    );
    assert!(!fake.exists(ROSIE_PACK));
}

#[test]
fn pull_refuses_a_pack_name_already_installed_from_another_owner() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
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

#[test]
fn pull_refuses_a_pack_name_installed_under_another_case() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let respelled = ["chakrit/Rosie", "someone/ROSIE"];
    let network = respelled.iter().fold(
        Canned::serving(ROSIE_URL, tarball.clone()),
        |network, text| network.and(&url_of(text), tarball.clone()),
    );
    packs::pull(&store, &network, &Source::default(), at(1)).expect("first pull");

    for text in respelled {
        let result = packs::pull(&store, &network, &source(text), at(2));

        assert!(
            matches!(&result, Err(Error::PackNameTaken { installed, .. }) if *installed == Source::default()),
            "{text}: got {result:?}"
        );
    }
    assert_eq!(listing(&fake, &format!("{DATA}/packs")), vec!["chakrit"]);
    assert_eq!(
        listing(&fake, &format!("{DATA}/packs/chakrit")),
        vec!["rosie"]
    );
    assert_eq!(store.installed().expect("list packs")[0].pulled_at, at(1));
}

// GitHub owner names are case-insensitive, so an owner the volume stores under another
// spelling is that owner's folder: the pull installs and reports the stored spelling.

#[test]
fn pull_replaces_a_pack_whose_owner_is_stored_in_another_case() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
    let first = Tarball::pack(&[("a.toml", "# old a")]).gzip();
    let second = Tarball::pack(&[("b.toml", "# new b")]).gzip();
    let stored = source("Chakrit/rosie");
    packs::pull(
        &store,
        &Canned::serving(&url_of("Chakrit/rosie"), first),
        &stored,
        at(1),
    )
    .expect("first pull");
    let network = Canned::serving(ROSIE_URL, second.clone()).and(&url_of("Chakrit/rosie"), second);

    let pulled = packs::pull(&store, &network, &Source::default(), at(2)).expect("default pull");

    assert_eq!(pulled.source, stored);
    assert_eq!(
        store.installed().expect("list packs"),
        vec![Installed {
            source: stored,
            pulled_at: at(2),
        }]
    );
    assert_eq!(listing(&fake, &format!("{DATA}/packs")), vec!["Chakrit"]);
    let pack = format!("{DATA}/packs/Chakrit/rosie");
    assert!(!fake.exists(format!("{pack}/a.toml")));
    assert_eq!(contents(&fake, format!("{pack}/b.toml")), "# new b");
}

#[test]
fn pull_installs_a_new_pack_under_the_owner_as_stored() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let network = ["Chakrit/rosie", "chakrit/other", "Chakrit/other"]
        .iter()
        .fold(Canned::offline(), |network, text| {
            network.and(&url_of(text), tarball.clone())
        });
    packs::pull(&store, &network, &source("Chakrit/rosie"), at(1)).expect("first pull");

    let first = packs::pull(&store, &network, &source("chakrit/other"), at(2));
    let second = packs::pull(&store, &network, &source("chakrit/other"), at(3));

    for pulled in [first, second] {
        let pulled = pulled.expect("pull under the stored owner");
        assert_eq!(pulled.source, source("Chakrit/other"));
    }
    assert_eq!(
        store.sources().expect("list packs"),
        vec![source("Chakrit/other"), source("Chakrit/rosie")]
    );
}

/// `İ` lowercases to `i` and a combining dot, so the fake folds 33 of them onto a name
/// of 66 characters, which is too long to be an owner.
#[test]
fn pull_refuses_an_owner_the_volume_stores_under_an_invalid_name() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
    let invalid = "i\u{307}".repeat(33);
    let stray = format!("{DATA}/packs/{invalid}");
    fake.add_dir(&stray);
    let requested = format!("{}/rosie", "\u{130}".repeat(33));
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let network = Canned::serving(&url_of(&requested), tarball);

    let result = packs::pull(&store, &network, &source(&requested), at(1));

    assert!(
        matches!(&result, Err(Error::StrayOwnerFolder { path, .. }) if *path == Path::new(&stray)),
        "got {result:?}"
    );
    assert_eq!(listing(&fake, &stray), Vec::<std::ffi::OsString>::new());
}

// The fake folds case only; the same refusals under another Unicode form run against
// the real volume in `pack_names.rs`.

#[test]
fn pull_refuses_a_pack_the_volume_folds_onto_the_reserved_name_user() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let network = Canned::serving(&url_of("someone/User"), tarball);

    let result = packs::pull(&store, &network, &source("someone/User"), at(1));

    assert!(
        matches!(&result, Err(Error::ReservedPack { text }) if text == "someone/User"),
        "got {result:?}"
    );
    assert_eq!(store.sources().expect("list packs"), Vec::<Source>::new());
    assert!(!fake.exists(format!("{DATA}/packs/someone/.User.new")));
}

#[test]
fn pull_warns_about_rules_that_duplicate_another_packs_rules() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
    let rosie = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let duplicate = NODE_RULE.replace("node-modules", "npm-deps");
    let different = NODE_RULE
        .replace("node-modules", "vendored")
        .replace("\"node_modules\"", "\"vendor\"");
    let extra = Tarball::pack(&[("js.toml", &format!("{duplicate}\n{different}"))]).gzip();
    let network = Canned::serving(ROSIE_URL, rosie).and(&url_of("someone/extra"), extra);
    let rosie_pulled =
        packs::pull(&store, &network, &Source::default(), at(1)).expect("rosie pull");

    let extra_pulled =
        packs::pull(&store, &network, &source("someone/extra"), at(2)).expect("extra pull");
    let rosie_again =
        packs::pull(&store, &network, &Source::default(), at(3)).expect("rosie again");

    assert_eq!(rosie_pulled.conflicts, vec![]);
    let warnings: Vec<String> = extra_pulled
        .conflicts
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        warnings,
        vec!["extra/npm-deps and rosie/node-modules clean the same target the same way"]
    );
    assert_eq!(
        rosie_again.conflicts.len(),
        1,
        "the other pack still clashes"
    );
}

// default config

#[test]
fn downloads_the_default_config_from_the_default_source() {
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let network = Canned::serving(ROSIE_URL, tarball);

    let config = packs::default_config(&network).expect("config downloaded");

    assert_eq!(config, "roots = []\n");
    assert_eq!(network.requested(), vec![ROSIE_URL]);
}

// remove

#[test]
fn removes_any_pack_including_rosie() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
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
    assert_eq!(store.installed().expect("list packs"), vec![]);
}

#[test]
fn removing_a_pack_that_is_not_installed_fails() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));

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
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
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
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
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
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));

    let result = packs::first_run(&store, &Canned::offline(), at(5));

    let Err(error) = result else {
        panic!("expected the offline pull to fail, got {result:?}");
    };
    assert!(matches!(&error, Error::Download { url, .. } if url == ROSIE_URL));
    assert!(
        error.to_string().contains("the network is unreachable"),
        "the reason is named: {error}"
    );
    assert_eq!(store.installed().expect("list packs"), vec![]);
}

#[test]
fn removing_the_last_pack_makes_the_next_run_pull_rosie_again() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, Path::new(DATA));
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

fn installed(pulled_at: SystemTime) -> Installed {
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
        packs::age_warning(&[installed(pulled_at)], six_months - Duration::from_secs(1));
    let at_six = packs::age_warning(&[installed(pulled_at)], six_months);

    assert_eq!(just_before, None);
    assert_eq!(
        at_six.map(|warning| warning.to_string()),
        Some("Your rules are 6 months old, consider rosie rules pull to update".to_owned())
    );
}

#[test]
fn ages_the_rules_by_the_oldest_pack() {
    let now = at(100_000_000);
    let packs = [installed(now - MONTH * 2), installed(now - MONTH * 13)];

    let warning = packs::age_warning(&packs, now);

    assert_eq!(
        warning.map(|warning| warning.to_string()),
        Some("Your rules are 13 months old, consider rosie rules pull to update".to_owned())
    );
}

#[test]
fn a_pull_time_in_the_future_is_not_stale() {
    let now = at(1_000);

    let warning = packs::age_warning(&[installed(now + MONTH * 12)], now);

    assert_eq!(warning, None);
}
