//! Fixture tier for rules: loading the layers into the in-memory fake, and matching
//! candidate folders (`docs/spec/rules.md`, `docs/spec/testing.md#tiers`).

use std::ffi::{OsStr, OsString};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use rosie::fs::fake::{FakeBackend, USER_UID};
use rosie::fs::{Bounds, Gate, Op};
use rosie::rules::{
    Candidate, Error, LuaSandbox, Mode, Problem, RuleDirs, RuleId, RuleSet, Shape, Source, Tier,
    Twin,
};

const HOME: &str = "/Users/me";
const PACKS: &str = "/Users/me/.local/share/rosie/packs";
const USER_RULES: &str = "/Users/me/.config/rosie/rules";
const ROSIE_PACK: &str = "/Users/me/.local/share/rosie/packs/chakrit/rosie";

fn bounds() -> Bounds {
    Bounds {
        roots: vec![PathBuf::from("/Users/me/code")],
        config_dir: PathBuf::from("/Users/me/.config/rosie"),
        data_dir: PathBuf::from("/Users/me/.local/share/rosie"),
        user_uid: USER_UID,
    }
}

fn gate(fake: &FakeBackend) -> Gate<&FakeBackend> {
    Gate::new(fake, bounds()).expect("absolute bounds")
}

fn load(fake: &FakeBackend) -> Result<RuleSet, Error> {
    RuleSet::load(&gate(fake), &RuleDirs::new(&bounds()))
}

fn loaded(fake: &FakeBackend) -> RuleSet {
    load(fake).expect("rules load")
}

fn id(text: &str) -> RuleId {
    RuleId::parse_qualified(text).expect("qualified name")
}

fn ids(rules: &RuleSet) -> Vec<String> {
    rules.iter().map(|rule| rule.id().to_string()).collect()
}

fn shape<'a>(rules: &'a RuleSet, text: &str) -> &'a Shape {
    let wanted = id(text);
    let rule = rules.iter().find(|rule| *rule.id() == wanted);
    rule.expect("rule loaded").shape()
}

/// The rules that accept `path`, with their tier, given the names in its parent.
fn matching(fake: &FakeBackend, rules: &RuleSet, path: &str) -> Vec<(String, Tier)> {
    let gate = gate(fake);
    let lua = LuaSandbox::new().expect("lua starts");
    let path = Path::new(path);
    let siblings: Vec<OsString> = gate
        .read_dir(path.parent().expect("has a parent"))
        .expect("parent lists");
    let candidate = Candidate {
        path,
        siblings: &siblings,
    };

    let name = path.file_name().expect("has a name");
    rules
        .targeting(name)
        .iter()
        .filter(|rule| {
            rule.matches(&gate, &lua, candidate)
                .expect("detection runs")
        })
        .map(|rule| (rule.id().to_string(), rule.tier()))
        .collect()
}

fn names(matches: &[(String, Tier)]) -> Vec<&str> {
    matches.iter().map(|(id, _)| id.as_str()).collect()
}

// layers

#[test]
fn loads_nothing_when_no_pack_or_user_rule_exists() {
    let fake = FakeBackend::new();
    fake.add_dir(HOME);

    assert!(loaded(&fake).is_empty());
}

#[test]
fn names_pulled_rules_by_repo_and_user_rules_by_the_user_pack() {
    let fake = FakeBackend::new();
    fake.add_file(
        format!("{ROSIE_PACK}/node.toml"),
        "[rules.npm-cache]\nstrategy = \"path\"\npaths = [\"~/.npm\"]",
    );
    fake.add_file(
        format!("{PACKS}/someone/extra-rules/go.toml"),
        "[rules.go-build]\nstrategy = \"path\"\npaths = [\"~/Library/Caches/go-build\"]",
    );
    fake.add_file(
        format!("{USER_RULES}/mine.toml"),
        "[rules.scratch]\nstrategy = \"name\"\ntarget = \"scratch-out\"",
    );
    fake.add_file(format!("{PACKS}/.DS_Store"), "");
    fake.add_file(format!("{ROSIE_PACK}/README.md"), "not a rule file");

    let rules = loaded(&fake);

    assert_eq!(
        ids(&rules),
        vec!["extra-rules/go-build", "rosie/npm-cache", "user/scratch"]
    );
    assert!(rules.iter().all(|rule| rule.source() == Source::Pack));
}

#[test]
fn skips_the_pulled_marker_and_dot_named_staging_and_aside_copies() {
    let fake = FakeBackend::new();
    fake.add_file(
        format!("{ROSIE_PACK}/node.toml"),
        "[rules.npm-cache]\nstrategy = \"path\"\npaths = [\"~/.npm\"]",
    );
    fake.add_file(format!("{ROSIE_PACK}/.pulled"), "1000");
    fake.add_file(
        format!("{PACKS}/chakrit/.rosie.new/broken.toml"),
        "[rules.x\n",
    );
    fake.add_dir(format!("{PACKS}/chakrit/.rosie.old"));
    fake.add_dir(format!("{PACKS}/chakrit/.rosie.removed"));

    let rules = loaded(&fake);

    assert_eq!(ids(&rules), vec!["rosie/npm-cache"]);
}

#[test]
fn a_user_override_replaces_the_whole_rule_under_its_identity() {
    let fake = FakeBackend::new();
    fake.add_file(
        format!("{ROSIE_PACK}/docker.toml"),
        "[rules.docker]\nstrategy = \"tool\"\ncmd = \"docker system prune --force\"\n\
         cmd_aggressive = \"docker system prune --all --volumes --force\"",
    );
    fake.add_file(
        format!("{USER_RULES}/docker.toml"),
        "[rules.\"rosie/docker\"]\nstrategy = \"path\"\npaths = [\"~/.docker/buildx\"]",
    );

    let rules = loaded(&fake);

    assert_eq!(ids(&rules), vec!["rosie/docker"]);
    let docker = rules.iter().next().expect("one rule");
    assert_eq!(docker.source(), Source::UserOverride);
    assert!(
        matches!(docker.shape(), Shape::Paths(Twin::Normal(_))),
        "the override replaces the whole rule, twin included: {:?}",
        docker.shape()
    );
}

#[test]
fn refuses_an_override_of_a_rule_no_pack_defines() {
    let fake = FakeBackend::new();
    fake.add_file(
        format!("{USER_RULES}/x.toml"),
        "[rules.\"rosie/nope\"]\nstrategy = \"name\"\ntarget = \"x\"",
    );

    let result = load(&fake);

    assert!(
        matches!(&result, Err(Error::UnknownOverride { rule, .. }) if *rule == id("rosie/nope")),
        "{:?}",
        result.err()
    );
}

#[test]
fn refuses_a_qualified_override_of_the_users_own_rule() {
    let fake = FakeBackend::new();
    fake.add_file(
        format!("{USER_RULES}/a.toml"),
        "[rules.x]\nstrategy = \"name\"\ntarget = \"x\"",
    );
    fake.add_file(
        format!("{USER_RULES}/b.toml"),
        "[rules.\"user/x\"]\nstrategy = \"name\"\ntarget = \"y\"",
    );

    let result = load(&fake);

    assert!(
        matches!(&result, Err(Error::UnknownOverride { rule, .. }) if *rule == id("user/x")),
        "{:?}",
        result.err()
    );
}

#[test]
fn refuses_a_pack_rule_that_overrides_another_pack() {
    let fake = FakeBackend::new();
    fake.add_file(
        format!("{ROSIE_PACK}/docker.toml"),
        "[rules.docker]\nstrategy = \"tool\"\ncmd = \"docker system prune\"",
    );
    fake.add_file(
        format!("{PACKS}/evil/pack/x.toml"),
        "[rules.\"rosie/docker\"]\nstrategy = \"tool\"\ncmd = \"curl evil\"",
    );

    let result = load(&fake);

    assert!(
        matches!(&result, Err(Error::Rule { rule, problem: Problem::OverrideInPack, .. }) if rule == "rosie/docker"),
        "{:?}",
        result.err()
    );
}

#[test]
fn refuses_rules_and_packs_defined_twice() {
    let twice_in_pack = FakeBackend::new();
    twice_in_pack.add_file(
        format!("{ROSIE_PACK}/a.toml"),
        "[rules.x]\nstrategy = \"name\"\ntarget = \"x\"",
    );
    twice_in_pack.add_file(
        format!("{ROSIE_PACK}/b.toml"),
        "[rules.x]\nstrategy = \"name\"\ntarget = \"y\"",
    );
    let same_pack_name = FakeBackend::new();
    same_pack_name.add_dir(ROSIE_PACK);
    same_pack_name.add_dir(format!("{PACKS}/fork/rosie"));
    let user_pack = FakeBackend::new();
    user_pack.add_dir(format!("{PACKS}/someone/user"));
    let overridden_twice = FakeBackend::new();
    overridden_twice.add_file(
        format!("{ROSIE_PACK}/a.toml"),
        "[rules.x]\nstrategy = \"name\"\ntarget = \"x\"",
    );
    for file in ["one", "two"] {
        overridden_twice.add_file(
            format!("{USER_RULES}/{file}.toml"),
            "[rules.\"rosie/x\"]\nstrategy = \"name\"\ntarget = \"z\"",
        );
    }

    assert!(matches!(
        load(&twice_in_pack),
        Err(Error::DuplicateRule { rule, .. }) if rule == id("rosie/x")
    ));
    assert!(matches!(
        load(&same_pack_name),
        Err(Error::DuplicatePack { pack, .. }) if pack == "rosie"
    ));
    assert!(matches!(
        load(&user_pack),
        Err(Error::DuplicatePack { pack, .. }) if pack == "user"
    ));
    assert!(matches!(
        load(&overridden_twice),
        Err(Error::DuplicateRule { rule, .. }) if rule == id("rosie/x")
    ));
}

#[test]
fn refuses_invalid_pack_and_rule_names() {
    let bad_pack = FakeBackend::new();
    bad_pack.add_file(
        format!("{PACKS}/someone/my.rules/x.toml"),
        "[rules.x]\nstrategy = \"name\"\ntarget = \"x\"",
    );
    let bad_rule = FakeBackend::new();
    bad_rule.add_file(
        format!("{USER_RULES}/x.toml"),
        "[rules.\"my rule\"]\nstrategy = \"name\"\ntarget = \"x\"",
    );

    assert!(matches!(load(&bad_pack), Err(Error::PackName { .. })));
    assert!(matches!(
        load(&bad_rule),
        Err(Error::Rule { rule, problem: Problem::Name(_), .. }) if rule == "my rule"
    ));
}

#[test]
fn names_the_file_and_rule_of_an_invalid_rule() {
    let fake = FakeBackend::new();
    fake.add_file(
        format!("{ROSIE_PACK}/lua.toml"),
        "[rules.odd]\nstrategy = \"lua\"\ntarget = \"x\"\nexpr = \"true\"\nscript = \"return true\"",
    );

    let error = load(&fake).err().expect("load fails");

    assert_eq!(
        error.to_string(),
        format!(
            "{ROSIE_PACK}/lua.toml: rule `odd`: needs exactly one of `expr` or `script`, not both"
        )
    );
}

#[test]
fn refuses_rule_files_and_packs_behind_symlinks() {
    let linked_file = FakeBackend::new();
    linked_file.add_file("/Users/me/dotfiles/node.toml", "");
    linked_file.add_symlink(
        format!("{ROSIE_PACK}/node.toml"),
        "/Users/me/dotfiles/node.toml",
    );
    let linked_pack = FakeBackend::new();
    linked_pack.add_dir("/Users/me/dotfiles/pack");
    linked_pack.add_symlink(format!("{PACKS}/me/pack"), "/Users/me/dotfiles/pack");
    let linked_user_dir = FakeBackend::new();
    linked_user_dir.add_file("/Users/me/dotfiles/rules/x.toml", "");
    linked_user_dir.add_symlink(USER_RULES, "/Users/me/dotfiles/rules");

    assert!(matches!(load(&linked_file), Err(Error::WrongKind { .. })));
    assert!(matches!(load(&linked_pack), Err(Error::WrongKind { .. })));
    assert!(matches!(
        load(&linked_user_dir),
        Err(Error::Fs(rosie::fs::Error::Symlink { .. }))
    ));
}

#[test]
fn refuses_a_packs_or_rules_path_that_is_not_a_folder() {
    let packs_file = FakeBackend::new();
    packs_file.add_file(PACKS, "");
    let rules_file = FakeBackend::new();
    rules_file.add_file(USER_RULES, "");

    for fake in [packs_file, rules_file] {
        let result = load(&fake);
        assert!(
            matches!(
                &result,
                Err(Error::WrongKind {
                    found: "file",
                    expected: "folder",
                    ..
                })
            ),
            "{:?}",
            result.err()
        );
    }
}

#[test]
fn reports_a_failed_inspection_of_a_rules_folder() {
    let fake = FakeBackend::new();
    fake.add_dir(PACKS);
    fake.fail_on(PACKS, Op::Lstat, ErrorKind::PermissionDenied);

    let result = load(&fake);

    assert!(
        matches!(&result, Err(Error::Fs(rosie::fs::Error::Io { op: Op::Lstat, path, .. }))
            if path == Path::new(PACKS)),
        "{:?}",
        result.err()
    );
}

#[test]
fn reports_a_failed_read_naming_the_file() {
    let fake = FakeBackend::new();
    fake.add_file(format!("{ROSIE_PACK}/node.toml"), "");
    fake.fail_on(
        format!("{ROSIE_PACK}/node.toml"),
        Op::ReadFile,
        ErrorKind::PermissionDenied,
    );

    let result = load(&fake);

    assert!(matches!(
        result,
        Err(Error::Fs(rosie::fs::Error::Io { op: Op::ReadFile, path, .. }))
            if path == Path::new(ROSIE_PACK).join("node.toml")
    ));
}

// modes

#[test]
fn infers_modes_from_the_rule_shape() {
    let fake = FakeBackend::new();
    fake.add_file(
        format!("{ROSIE_PACK}/all.toml"),
        "[rules.folder]\nstrategy = \"marker\"\ntarget = \"target\"\nmarker = [\"Cargo.toml\"]\n\
         [rules.cache]\nstrategy = \"path\"\npaths = [\"~/.npm\"]\n\
         [rules.tool]\nstrategy = \"tool\"\ncmd = \"docker system prune\"",
    );

    let rules = loaded(&fake);

    assert_eq!(shape(&rules, "rosie/folder").modes(), &[Mode::Tree]);
    assert_eq!(
        shape(&rules, "rosie/cache").modes(),
        &[Mode::Caches, Mode::Tree]
    );
    assert_eq!(shape(&rules, "rosie/tool").modes(), &[Mode::Caches]);
}

// matching

fn project_fixture() -> FakeBackend {
    let fake = FakeBackend::new();
    fake.add_file(
        format!("{ROSIE_PACK}/rules.toml"),
        "[rules.pycache]\nstrategy = \"name\"\ntarget = \"__pycache__\"\n\
         [rules.cargo-target]\nstrategy = \"marker\"\ntarget = \"target\"\nmarker = [\"Cargo.toml\"]\n\
         [rules.terraform]\nstrategy = \"marker\"\ntarget = \".terraform\"\nmarker = [\"*.tf\"]\n\
         [rules.venv]\nstrategy = \"marker\"\ntarget = \".venv\"\n\
         marker = [\"pyproject.toml\", \"requirements.txt\"]\ninside = [\"pyvenv.cfg\"]\n\
         [rules.pods]\nstrategy = \"marker\"\ntarget = \"Pods\"\ninside = [\"Manifest.lock\"]\n\
         [rules.derived]\nstrategy = \"name\"\ntarget = \"build\"\ntarget_aggressive = \"DerivedData\"\n\
         [rules.gradle]\nstrategy = \"lua\"\ntarget = \"build\"\n\
         expr = \"exists(parent(path) .. '/build.gradle')\"",
    );
    fake
}

#[test]
fn a_name_rule_matches_by_name_alone() {
    let fake = project_fixture();
    fake.add_dir("/Users/me/code/py/__pycache__");
    let rules = loaded(&fake);

    let found = matching(&fake, &rules, "/Users/me/code/py/__pycache__");

    assert_eq!(found, vec![("rosie/pycache".to_string(), Tier::Normal)]);
}

#[test]
fn a_marker_rule_needs_a_sibling_marker() {
    let fake = project_fixture();
    fake.add_file("/Users/me/code/rust/Cargo.toml", "");
    fake.add_dir("/Users/me/code/rust/target");
    fake.add_dir("/Users/me/code/java/target");
    fake.add_dir("/Users/me/code/java/Cargo.toml.d");
    let rules = loaded(&fake);

    let rust = matching(&fake, &rules, "/Users/me/code/rust/target");
    let java = matching(&fake, &rules, "/Users/me/code/java/target");

    assert_eq!(names(&rust), vec!["rosie/cargo-target"]);
    assert_eq!(names(&java), Vec::<&str>::new());
}

#[test]
fn a_marker_never_matches_the_folder_itself() {
    let fake = FakeBackend::new();
    fake.add_file(
        format!("{ROSIE_PACK}/dist.toml"),
        "[rules.dist]\nstrategy = \"marker\"\ntarget = \"dist\"\nmarker = [\"dist*\"]",
    );
    fake.add_dir("/Users/me/code/alone/dist");
    fake.add_dir("/Users/me/code/paired/dist");
    fake.add_file("/Users/me/code/paired/dist.config.js", "");
    let rules = loaded(&fake);

    assert!(matching(&fake, &rules, "/Users/me/code/alone/dist").is_empty());
    assert_eq!(
        names(&matching(&fake, &rules, "/Users/me/code/paired/dist")),
        vec!["rosie/dist"]
    );
}

#[test]
fn a_marker_glob_matches_any_sibling() {
    let fake = project_fixture();
    fake.add_file("/Users/me/code/infra/main.tf", "");
    fake.add_dir("/Users/me/code/infra/.terraform");
    fake.add_dir("/Users/me/code/bare/.terraform");
    fake.add_file("/Users/me/code/bare/main.tf.bak", "");
    let rules = loaded(&fake);

    assert_eq!(
        names(&matching(&fake, &rules, "/Users/me/code/infra/.terraform")),
        vec!["rosie/terraform"]
    );
    assert!(matching(&fake, &rules, "/Users/me/code/bare/.terraform").is_empty());
}

#[test]
fn marker_and_inside_must_both_match_when_both_are_set() {
    let fake = project_fixture();
    fake.add_file("/Users/me/code/both/requirements.txt", "");
    fake.add_file("/Users/me/code/both/.venv/pyvenv.cfg", "");
    fake.add_file("/Users/me/code/marker-only/pyproject.toml", "");
    fake.add_dir("/Users/me/code/marker-only/.venv");
    fake.add_file("/Users/me/code/inside-only/.venv/pyvenv.cfg", "");
    let rules = loaded(&fake);

    assert_eq!(
        names(&matching(&fake, &rules, "/Users/me/code/both/.venv")),
        vec!["rosie/venv"]
    );
    assert!(matching(&fake, &rules, "/Users/me/code/marker-only/.venv").is_empty());
    assert!(matching(&fake, &rules, "/Users/me/code/inside-only/.venv").is_empty());
}

#[test]
fn an_inside_rule_looks_within_the_folder() {
    let fake = project_fixture();
    fake.add_file("/Users/me/code/ios/Pods/Manifest.lock", "");
    fake.add_file("/Users/me/code/other/Pods/readme.txt", "");
    let rules = loaded(&fake);

    assert_eq!(
        names(&matching(&fake, &rules, "/Users/me/code/ios/Pods")),
        vec!["rosie/pods"]
    );
    assert!(matching(&fake, &rules, "/Users/me/code/other/Pods").is_empty());
}

#[test]
fn a_lua_rule_matches_what_its_code_accepts() {
    let fake = project_fixture();
    fake.add_file("/Users/me/code/android/build.gradle", "");
    fake.add_dir("/Users/me/code/android/build");
    let rules = loaded(&fake);

    let found = matching(&fake, &rules, "/Users/me/code/android/build");

    assert_eq!(names(&found), vec!["rosie/derived", "rosie/gradle"]);
}

#[test]
fn indexes_aggressive_targets_with_their_tier() {
    let fake = project_fixture();
    fake.add_dir("/Users/me/Library/Developer/Xcode/DerivedData");
    fake.add_dir("/Users/me/code/web/build");
    let rules = loaded(&fake);

    let aggressive = matching(
        &fake,
        &rules,
        "/Users/me/Library/Developer/Xcode/DerivedData",
    );
    let normal = matching(&fake, &rules, "/Users/me/code/web/build");

    assert_eq!(
        aggressive,
        vec![("rosie/derived".to_string(), Tier::Aggressive)]
    );
    assert_eq!(normal, vec![("rosie/derived".to_string(), Tier::Normal)]);
    assert!(rules.targeting(OsStr::new("node_modules")).is_empty());
}

#[test]
fn reports_a_lua_error_against_the_rule_without_matching() {
    let fake = FakeBackend::new();
    fake.add_file(
        format!("{ROSIE_PACK}/lua.toml"),
        "[rules.broken]\nstrategy = \"lua\"\ntarget = \"out\"\n\
         expr = \"read_json(parent(path) .. '/missing.json').x\"",
    );
    fake.add_dir("/Users/me/code/app/out");
    let rules = loaded(&fake);
    let gate = gate(&fake);
    let lua = LuaSandbox::new().expect("lua starts");
    let siblings = [OsString::from("out")];
    let candidate = Candidate {
        path: Path::new("/Users/me/code/app/out"),
        siblings: &siblings,
    };

    let entry = &rules.targeting(OsStr::new("out"))[0];
    let result = entry.matches(&gate, &lua, candidate);

    assert!(
        matches!(&result, Err(Error::Lua { rule, path, .. })
            if *rule == id("rosie/broken") && path == Path::new("/Users/me/code/app/out")),
        "{result:?}"
    );
}

#[test]
fn reports_a_failed_listing_against_the_rule_without_matching() {
    let fake = project_fixture();
    fake.add_dir("/Users/me/code/ios/Pods");
    fake.fail_on(
        "/Users/me/code/ios/Pods",
        Op::ReadDir,
        ErrorKind::PermissionDenied,
    );
    let rules = loaded(&fake);
    let gate = gate(&fake);
    let lua = LuaSandbox::new().expect("lua starts");
    let siblings = [OsString::from("Pods")];
    let candidate = Candidate {
        path: Path::new("/Users/me/code/ios/Pods"),
        siblings: &siblings,
    };

    let entry = &rules.targeting(OsStr::new("Pods"))[0];
    let result = entry.matches(&gate, &lua, candidate);

    assert!(
        matches!(&result, Err(Error::Listing { rule, path, .. })
            if *rule == id("rosie/pods") && path == Path::new("/Users/me/code/ios/Pods")),
        "{result:?}"
    );
}

// the built-in pack

#[test]
fn loads_the_repository_rule_pack() {
    let pack = Path::new(env!("CARGO_MANIFEST_DIR")).join("rules");
    let fake = FakeBackend::new();
    let entries = std::fs::read_dir(&pack).expect("rules/ lists");
    for entry in entries {
        let path = entry.expect("entry").path();
        let contents = std::fs::read(&path).expect("rule file reads");
        let name = path.file_name().expect("file name");
        fake.add_file(Path::new(ROSIE_PACK).join(name), contents);
    }

    let rules = loaded(&fake);

    assert!(!rules.is_empty());
    assert!(rules.iter().all(|rule| rule.id().pack.as_str() == "rosie"));
    assert!(!rules.targeting(OsStr::new("node_modules")).is_empty());
}
