//! Pulling, replacing, removing, and aging rule packs (`docs/spec/rules.md#packs`),
//! against the in-memory fake and a canned network. What an interrupted pull or remove
//! leaves behind is covered in `pack_recovery.rs`.

mod pack_fixture;
mod tarball;

use std::path::Path;
use std::time::{Duration, SystemTime};

use pack_fixture::{
    Canned, DATA, NODE_RULE, ROSIE_PACK, ROSIE_URL, at, contents, fake_home, gate, home, listing,
    source, url_of,
};
use rosie::packs::{self, Error, FirstRun, Installed, MONTH, RemovePack, Source, Store};
use rosie::rules::{self, Problem};
use tarball::{TOP, Tarball};

// pull

#[test]
fn pull_installs_only_the_rules_folder_into_the_pack_folder() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    assert_eq!(store.installed().expect("list packs")[0].pulled_at, at(1));
}

#[test]
fn pull_refuses_a_pack_name_already_installed_from_another_owner() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let store = Store::new(&gate, &home(), Path::new(DATA));
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

#[test]
fn pull_warns_when_a_duplicate_rules_markers_are_written_in_a_different_order() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
    let rosie = Tarball::pack(&[(
        "node.toml",
        "[rules.node-modules]\nstrategy = \"marker\"\ntarget = \"node_modules\"\n\
         marker = [\"package.json\", \"yarn.lock\"]\n",
    )])
    .gzip();
    let extra = Tarball::pack(&[(
        "js.toml",
        "[rules.npm-deps]\nstrategy = \"marker\"\ntarget = \"node_modules\"\n\
         marker = [\"yarn.lock\", \"package.json\"]\n",
    )])
    .gzip();
    let network = Canned::serving(ROSIE_URL, rosie).and(&url_of("someone/extra"), extra);
    packs::pull(&store, &network, &Source::default(), at(1)).expect("rosie pull");

    let extra_pulled =
        packs::pull(&store, &network, &source("someone/extra"), at(2)).expect("extra pull");

    let warnings: Vec<String> = extra_pulled
        .conflicts
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        warnings,
        vec!["extra/npm-deps and rosie/node-modules clean the same target the same way"]
    );
}

#[test]
fn pull_warns_when_the_installed_rules_markers_are_written_in_a_different_order() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
    let rosie = Tarball::pack(&[(
        "node.toml",
        "[rules.node-modules]\nstrategy = \"marker\"\ntarget = \"node_modules\"\n\
         marker = [\"yarn.lock\", \"package.json\"]\n",
    )])
    .gzip();
    let extra = Tarball::pack(&[(
        "js.toml",
        "[rules.npm-deps]\nstrategy = \"marker\"\ntarget = \"node_modules\"\n\
         marker = [\"package.json\", \"yarn.lock\"]\n",
    )])
    .gzip();
    let network = Canned::serving(ROSIE_URL, rosie).and(&url_of("someone/extra"), extra);
    packs::pull(&store, &network, &Source::default(), at(1)).expect("rosie pull");

    let extra_pulled =
        packs::pull(&store, &network, &source("someone/extra"), at(2)).expect("extra pull");

    let warnings: Vec<String> = extra_pulled
        .conflicts
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        warnings,
        vec!["extra/npm-deps and rosie/node-modules clean the same target the same way"]
    );
}

#[test]
fn pull_does_not_warn_when_only_one_of_sibling_and_inside_markers_matches() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
    let rosie = Tarball::pack(&[(
        "py.toml",
        "[rules.venv]\nstrategy = \"marker\"\ntarget = \".venv\"\n\
         marker = [\"pyproject.toml\"]\ninside = [\"pyvenv.cfg\"]\n",
    )])
    .gzip();
    let extra = Tarball::pack(&[(
        "py.toml",
        "[rules.venv]\nstrategy = \"marker\"\ntarget = \".venv\"\n\
         marker = [\"setup.py\"]\ninside = [\"pyvenv.cfg\"]\n",
    )])
    .gzip();
    let network = Canned::serving(ROSIE_URL, rosie).and(&url_of("someone/extra"), extra);
    packs::pull(&store, &network, &Source::default(), at(1)).expect("rosie pull");

    let extra_pulled =
        packs::pull(&store, &network, &source("someone/extra"), at(2)).expect("extra pull");

    assert_eq!(
        extra_pulled.conflicts,
        vec![],
        "the sibling markers differ, so it is not the same detection"
    );
}

#[test]
fn pull_warns_only_when_duplicate_lua_rules_run_the_same_code() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
    let rosie = Tarball::pack(&[(
        "lua.toml",
        "[rules.odd]\nstrategy = \"lua\"\ntarget = \"x\"\nexpr = \"true\"",
    )])
    .gzip();
    let extra = Tarball::pack(&[(
        "lua.toml",
        "[rules.same]\nstrategy = \"lua\"\ntarget = \"x\"\nexpr = \"true\"\n\
         [rules.different]\nstrategy = \"lua\"\ntarget = \"x\"\nexpr = \"false\"",
    )])
    .gzip();
    let network = Canned::serving(ROSIE_URL, rosie).and(&url_of("someone/extra"), extra);
    packs::pull(&store, &network, &Source::default(), at(1)).expect("rosie pull");

    let extra_pulled =
        packs::pull(&store, &network, &source("someone/extra"), at(2)).expect("extra pull");

    let warnings: Vec<String> = extra_pulled
        .conflicts
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        warnings,
        vec!["extra/same and rosie/odd clean the same target the same way"],
        "only the rule with identical Lua code should clash"
    );
}

#[test]
fn pull_does_not_warn_when_only_one_side_of_a_marker_rule_names_inside() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
    let rosie = Tarball::pack(&[(
        "venv.toml",
        "[rules.venv]\nstrategy = \"marker\"\ntarget = \"t\"\nmarker = [\"p\"]",
    )])
    .gzip();
    let extra = Tarball::pack(&[(
        "venv.toml",
        "[rules.venv]\nstrategy = \"marker\"\ntarget = \"t\"\nmarker = [\"p\"]\n\
         inside = [\"q\"]",
    )])
    .gzip();
    let network = Canned::serving(ROSIE_URL, rosie).and(&url_of("someone/extra"), extra);
    packs::pull(&store, &network, &Source::default(), at(1)).expect("rosie pull");

    let extra_pulled =
        packs::pull(&store, &network, &source("someone/extra"), at(2)).expect("extra pull");

    assert_eq!(
        extra_pulled.conflicts,
        vec![],
        "one side names `inside` markers the other side lacks, so it is not the same detection"
    );
}

/// Pulls the default pack with `node.toml`, then pulls it again as `files`, which must be
/// refused as invalid rules while the first pack stays installed. Returns the refusal's
/// inner rule error.
fn refused_pull_keeps_the_installed_pack(files: &[(&str, &str)]) -> rules::Error {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    assert_eq!(store.installed().expect("list packs")[0].pulled_at, at(1));
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

/// Pulls `rosie_rules` as the default pack, then `extra_rules` as `someone/extra`, and
/// returns the second pull's warnings.
fn warnings_pulling_beside(rosie_rules: &str, extra_rules: &str) -> Vec<String> {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
    let rosie = Tarball::pack(&[("rosie.toml", rosie_rules)]).gzip();
    let extra = Tarball::pack(&[("extra.toml", extra_rules)]).gzip();
    let network = Canned::serving(ROSIE_URL, rosie).and(&url_of("someone/extra"), extra);
    packs::pull(&store, &network, &Source::default(), at(1)).expect("rosie pull");

    let pulled =
        packs::pull(&store, &network, &source("someone/extra"), at(2)).expect("extra pull");

    pulled.conflicts.iter().map(ToString::to_string).collect()
}

#[test]
fn pull_compares_the_aggressive_target_too() {
    let rosie = "[rules.x]\nstrategy = \"name\"\ntarget = \"x\"\ntarget_aggressive = \"y\"";
    let extra = "[rules.other-aggressive]\nstrategy = \"name\"\ntarget = \"x\"\n\
                 target_aggressive = \"z\"\n\
                 [rules.same]\nstrategy = \"name\"\ntarget = \"x\"\ntarget_aggressive = \"y\"";

    let warnings = warnings_pulling_beside(rosie, extra);

    assert_eq!(
        warnings,
        vec!["extra/same and rosie/x clean the same target the same way"]
    );
}

#[test]
fn pull_warns_about_path_rules_sharing_a_resolved_path() {
    let rosie = "[rules.npm]\nstrategy = \"path\"\npaths = [\"~/.npm\"]";
    let extra = "[rules.absolute]\nstrategy = \"path\"\npaths = [\"/Users/me/.npm\"]\n\
                 [rules.aggressive]\nstrategy = \"path\"\npaths_aggressive = [\"~/.npm\"]\n\
                 [rules.folder]\nstrategy = \"name\"\ntarget = \".npm\"\n\
                 [rules.yarn]\nstrategy = \"path\"\npaths = [\"~/.yarn\", \"/Users/me/.npm2\"]\n\
                 [rules.multi]\nstrategy = \"path\"\npaths = [\"~/.other\", \"~/.npm\"]\n\
                 [rules.mixed]\nstrategy = \"path\"\npaths = [\"~/.elsewhere\"]\n\
                 paths_aggressive = [\"~/.npm\"]";

    let warnings = warnings_pulling_beside(rosie, extra);

    assert_eq!(
        warnings,
        vec![
            "extra/absolute and rosie/npm clean the same target the same way",
            "extra/aggressive and rosie/npm clean the same target the same way",
            "extra/mixed and rosie/npm clean the same target the same way",
            "extra/multi and rosie/npm clean the same target the same way",
        ]
    );
}

#[test]
fn pull_warns_about_tool_rules_running_the_same_command() {
    let rosie = "[rules.prune]\nstrategy = \"tool\"\ncmd = \"docker system prune\"";
    let extra = "[rules.spaced]\nstrategy = \"tool\"\ncmd = \"  docker system prune \"\n\
                 [rules.aggressive]\nstrategy = \"tool\"\ncmd_aggressive = \"docker system prune\"\n\
                 [rules.all]\nstrategy = \"tool\"\ncmd = \"docker system prune --all\"\n\
                 [rules.mixed]\nstrategy = \"tool\"\ncmd = \"docker builder prune\"\n\
                 cmd_aggressive = \"docker system prune\"";

    let warnings = warnings_pulling_beside(rosie, extra);

    assert_eq!(
        warnings,
        vec![
            "extra/aggressive and rosie/prune clean the same target the same way",
            "extra/mixed and rosie/prune clean the same target the same way",
            "extra/spaced and rosie/prune clean the same target the same way",
        ]
    );
}

#[test]
fn pull_does_not_warn_about_folder_rules_sharing_a_target_with_different_detection() {
    let rosie = "[rules.node-modules]\nstrategy = \"marker\"\ntarget = \"node_modules\"\n\
                 marker = [\"package.json\"]";
    let extra = "[rules.node-by-name]\nstrategy = \"name\"\ntarget = \"node_modules\"";

    let warnings = warnings_pulling_beside(rosie, extra);

    assert_eq!(warnings, Vec::<String>::new());
}

#[test]
fn pull_does_not_warn_about_folder_rules_with_different_targets_and_the_same_detection() {
    let rosie = "[rules.node-modules]\nstrategy = \"marker\"\ntarget = \"node_modules\"\n\
                 marker = [\"package.json\"]";
    let extra = "[rules.venv]\nstrategy = \"marker\"\ntarget = \".venv\"\n\
                 marker = [\"package.json\"]";

    let warnings = warnings_pulling_beside(rosie, extra);

    assert_eq!(
        warnings,
        Vec::<String>::new(),
        "the targets differ, so it is not the same folder"
    );
}

#[test]
fn pull_refuses_while_another_installed_pack_fails_to_load() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
    let tarball = Tarball::pack(&[("node.toml", NODE_RULE)]).gzip();
    let network =
        Canned::serving(ROSIE_URL, tarball.clone()).and(&url_of("someone/extra"), tarball);
    packs::pull(&store, &network, &source("someone/extra"), at(1)).expect("extra pull");
    let broken = format!("{DATA}/packs/someone/extra/broken.toml");
    fake.add_file(&broken, "[rules.x]\nstrategy = \"regex\"");

    let result = packs::pull(&store, &network, &Source::default(), at(2));

    let Err(Error::InvalidRules(inner)) = &result else {
        panic!("expected the broken pack to refuse the pull, got {result:?}");
    };
    assert!(
        matches!(&**inner, rules::Error::Rule { file, .. } if *file == Path::new("someone/extra/broken.toml")),
        "the broken pack's file is named: {inner}"
    );
    assert!(!fake.exists(ROSIE_PACK));
}

// seeding a config

#[test]
fn seeding_without_packs_installs_the_default_pack_from_the_one_download() {
    let fake = fake_home();
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
        store.installed().expect("list packs"),
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
    let gate = gate(&fake);
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let store = Store::new(&gate, &home(), Path::new(DATA));

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
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
    let store = Store::new(&gate, &home(), Path::new(DATA));

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
    let store = Store::new(&gate, &home(), Path::new(DATA));
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
