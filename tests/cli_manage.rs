//! Sandbox tier: the management commands, their pickers, and the app modes.

mod cli_fixture;
mod pack_fixture;
mod tarball;

use cli_fixture::{CONFIG_FILE, CWD, HOME, NODE_RULE, PACK, Sandbox, Script};
use pack_fixture::{Canned, ROSIE_SET_ASIDE, url_of};
use rosie::cli::ExitStatus;
use tarball::Tarball;

// rules

#[test]
fn rules_lists_each_rule_with_its_pack() {
    let sandbox = Sandbox::new();
    sandbox.fake.add_file(
        "/Users/me/.config/rosie/rules/mine.toml",
        "[rules.logs]\nstrategy = \"name\"\ntarget = \"logs\"\n",
    );

    let ran = sandbox.run(&["rules"]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(
        ran.stdout.lines().collect::<Vec<_>>(),
        ["node-modules (pack rosie)", "logs (pack user)"]
    );
}

#[test]
fn rules_pull_installs_a_named_source_and_rules_remove_takes_it_away() {
    let pack = Tarball::pack(&[(
        "go.toml",
        "[rules.go-vendor]\nstrategy = \"name\"\ntarget = \"vendor\"\n",
    )]);
    let sandbox = Sandbox::new().with_net(Canned::serving(&url_of("someone/extra"), pack.gzip()));

    let pulled = sandbox.run(&["rules", "pull", "someone/extra"]);
    let listed = sandbox.run(&["rules"]);
    let removed = sandbox.run(&["rules", "remove", "extra"]);

    pulled.assert_status(ExitStatus::Success);
    assert_eq!(pulled.stdout.trim(), "pulled someone/extra");
    assert!(
        listed.stdout.contains("go-vendor (pack extra)"),
        "{}",
        listed.stdout
    );
    removed.assert_status(ExitStatus::Success);
    assert_eq!(removed.stdout.trim(), "removed someone/extra");
    assert!(!sandbox.exists("/Users/me/.local/share/rosie/packs/someone/extra"));
    assert!(sandbox.exists(PACK));
}

#[test]
fn rules_pull_of_a_bad_or_reserved_source_shows_the_form_to_give() {
    let sandbox = Sandbox::new();

    let bad = sandbox.run(&["rules", "pull", "no-slash"]);
    let reserved = sandbox.run(&["rules", "pull", "someone/user"]);

    for ran in [bad, reserved] {
        assert!(
            ran.stderr.contains("rosie rules pull <owner/repo>"),
            "{}",
            ran.stderr
        );
    }
}

#[test]
fn rules_remove_without_a_pack_offers_the_pulled_packs() {
    let sandbox = Sandbox::new();

    let ran = sandbox.run_with(Script::terminal().pick(0), &["rules", "remove"]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(ran.asked, ["Choose a pack: rosie (chakrit/rosie)"]);
    assert!(!sandbox.exists(PACK));
}

/// A pull that failed mid-swap can leave the only pack at its set-aside copy, not at
/// its normal name; the bare picker settles it first, the same as the named form.
#[test]
fn rules_remove_without_a_pack_settles_a_set_aside_pack_first() {
    let sandbox = Sandbox::bare();
    sandbox.write_config(&format!("roots = [\"{HOME}\"]\n"));
    sandbox
        .fake
        .add_file(format!("{ROSIE_SET_ASIDE}/node.toml"), NODE_RULE);
    sandbox
        .fake
        .add_file(format!("{ROSIE_SET_ASIDE}/.pulled"), "1800000000");

    let ran = sandbox.run_with(Script::terminal().pick(0), &["rules", "remove"]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(ran.asked, ["Choose a pack: rosie (chakrit/rosie)"]);
    assert!(!sandbox.exists(PACK));
    assert!(!sandbox.exists(ROSIE_SET_ASIDE));
}

#[test]
fn rules_remove_with_no_packs_pulled_says_so() {
    let sandbox = Sandbox::bare();
    sandbox.write_config(&format!("roots = [\"{HOME}\"]\n"));

    let ran = sandbox.run_with(Script::terminal(), &["rules", "remove"]);

    ran.assert_status(ExitStatus::Failed);
    assert!(
        ran.stderr.contains("no pulled packs to choose from"),
        "{}",
        ran.stderr
    );
}

// roots

#[test]
fn roots_remove_with_no_roots_says_so() {
    let sandbox = Sandbox::with_roots(&[]);

    let ran = sandbox.run_with(Script::terminal(), &["roots", "remove"]);

    ran.assert_status(ExitStatus::Failed);
    assert!(
        ran.stderr.contains("no roots to choose from"),
        "{}",
        ran.stderr
    );
}

#[test]
fn roots_lists_adds_and_removes_roots() {
    let sandbox = Sandbox::new();
    sandbox.fake.add_dir(format!("{CWD}/project"));

    let added = sandbox.run(&["roots", "add", "project"]);
    let listed = sandbox.run(&["roots"]);
    let removed = sandbox.run(&["roots", "remove", HOME]);
    let after = sandbox.run(&["roots"]);

    added.assert_status(ExitStatus::Success);
    assert_eq!(listed.stdout, format!("{HOME}\n{CWD}/project\n"));
    removed.assert_status(ExitStatus::Success);
    assert_eq!(after.stdout, format!("{CWD}/project\n"));
}

#[test]
fn roots_add_refuses_a_symlinked_path_naming_the_real_one() {
    let sandbox = Sandbox::new();
    sandbox.fake.add_dir(format!("{CWD}/real"));
    sandbox.fake.add_symlink(format!("{CWD}/link"), "real");

    let ran = sandbox.run(&["roots", "add", "link"]);

    ran.assert_status(ExitStatus::Failed);
    assert!(
        ran.stderr.contains(&format!("{CWD}/real")),
        "{}",
        ran.stderr
    );
    assert_eq!(sandbox.file(CONFIG_FILE), format!("roots = [\"{HOME}\"]\n"));
}

#[test]
fn roots_remove_without_a_path_offers_the_roots() {
    let sandbox = Sandbox::with_roots(&[HOME, CWD]);

    let ran = sandbox.run_with(Script::terminal().pick(1), &["roots", "remove"]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(ran.asked, [format!("Choose a root: {HOME} | {CWD}")]);
    assert_eq!(sandbox.run(&["roots"]).stdout, format!("{HOME}\n"));
}

// config

#[test]
fn config_shows_every_key_and_value() {
    let sandbox = Sandbox::new();

    let ran = sandbox.run(&["config"]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(
        ran.stdout,
        format!(
            "roots =\n  {HOME}\nallow_outside_roots = false\nwalk.enter_bundles = false\nwalk.enter_mounts = false\nwalk.enter_placeholders = false\nwalk.enter_links = none\n"
        )
    );
}

#[test]
fn config_set_get_and_unset_a_key() {
    let sandbox = Sandbox::new();

    let set = sandbox.run(&["config", "set", "walk.enter_bundles", "true"]);
    let got = sandbox.run(&["config", "get", "walk.enter_bundles"]);
    let unset = sandbox.run(&["config", "unset", "walk.enter_bundles"]);
    let after = sandbox.run(&["config", "get", "walk.enter_bundles"]);

    set.assert_status(ExitStatus::Success);
    assert_eq!(got.stdout, "true\n");
    unset.assert_status(ExitStatus::Success);
    assert_eq!(after.stdout, "false\n");
}

#[test]
fn config_set_without_arguments_offers_the_keys_then_their_values() {
    let sandbox = Sandbox::new();
    let script = Script::terminal().pick(2).pick(0);

    let ran = sandbox.run_with(script, &["config", "set"]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(
        ran.asked,
        [
            "Choose a key: allow_outside_roots | walk.enter_bundles | walk.enter_mounts | walk.enter_placeholders | walk.enter_links",
            "Choose a value: true | false",
        ]
    );
    let got = sandbox.run(&["config", "get", "walk.enter_mounts"]);
    assert_eq!(got.stdout, "true\n");
}

#[test]
fn config_set_offers_the_enter_links_levels() {
    let sandbox = Sandbox::new();
    let script = Script::terminal().pick(4).pick(2);

    let ran = sandbox.run_with(script, &["config", "set"]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(ran.asked[1], "Choose a value: none | roots | all");
    let got = sandbox.run(&["config", "get", "walk.enter_links"]);
    assert_eq!(got.stdout, "all\n");
}

#[test]
fn config_get_without_a_key_offers_every_key() {
    let sandbox = Sandbox::new();

    let ran = sandbox.run_with(Script::terminal().pick(0), &["config", "get"]);

    ran.assert_status(ExitStatus::Success);
    assert!(
        ran.asked[0].starts_with("Choose a key: roots | allow_outside_roots | walk.enter_bundles")
    );
    assert_eq!(ran.stdout, format!("{HOME}\n"));
}

#[test]
fn config_refuses_an_unknown_key_and_roots() {
    let sandbox = Sandbox::new();

    let unknown = sandbox.run(&["config", "set", "walk.nope", "true"]);
    let roots = sandbox.run(&["config", "set", "roots", "true"]);

    unknown.assert_status(ExitStatus::Failed);
    roots.assert_status(ExitStatus::Failed);
    assert!(roots.stderr.contains("rosie roots add"), "{}", roots.stderr);
}

// app and orphans

const BAR: &str = "/Applications/Bar.app";

fn install_bar(sandbox: &Sandbox) {
    sandbox.install_app(BAR, "com.foo.Bar", "Bar");
    sandbox
        .fake
        .add_sized_file(format!("{BAR}/Contents/Resources/Assets.car"), 1 << 20);
    sandbox.fake.add_dir("/Users/me/Library/Caches/com.foo.Bar");
}

#[test]
fn scan_app_without_an_app_offers_the_installed_apps() {
    let sandbox = Sandbox::with_roots(&["/Applications", "/Users/me/Library"]);
    install_bar(&sandbox);

    let ran = sandbox.run_with(Script::terminal().pick(0), &["scan", "app", "--toml"]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(ran.asked, [format!("Choose an app: {BAR}")]);
    assert!(
        ran.stdout.contains(&format!("path = \"{BAR}\"")),
        "{}",
        ran.stdout
    );
    assert!(
        ran.stdout
            .contains("path = \"/Users/me/Library/Caches/com.foo.Bar\""),
        "{}",
        ran.stdout
    );
}

#[test]
fn scan_app_with_no_apps_installed_says_so() {
    let sandbox = Sandbox::with_roots(&["/Applications"]);

    let ran = sandbox.run_with(Script::terminal(), &["scan", "app"]);

    ran.assert_status(ExitStatus::Failed);
    assert!(
        ran.stderr.contains("no installed apps to choose from"),
        "{}",
        ran.stderr
    );
}

// app name matching (docs/spec/app.md#app-name-matching)

const VS_CODE: &str = "/Applications/Visual Studio Code.app";
const XCODE: &str = "/Applications/Xcode.app";

fn code_sandbox() -> Sandbox {
    let sandbox = Sandbox::with_roots(&["/Applications", "/Users/me/Library"]);
    sandbox.install_app(VS_CODE, "com.microsoft.VSCode", "Code");
    sandbox.install_app(XCODE, "com.apple.dt.Xcode", "Xcode");
    sandbox
}

#[test]
fn scan_app_finds_an_app_by_a_word_in_its_file_name() {
    let sandbox = code_sandbox();

    let ran = sandbox.run(&["scan", "app", "Studio", "--toml"]);

    ran.assert_status(ExitStatus::Success);
    assert!(ran.asked.is_empty(), "{:?}", ran.asked);
    assert!(
        ran.stdout.contains(&format!("path = \"{VS_CODE}\"")),
        "{}",
        ran.stdout
    );
    assert!(!ran.stdout.contains(XCODE), "{}", ran.stdout);
}

#[test]
fn scan_app_matches_a_file_name_pattern_ignoring_case() {
    let sandbox = code_sandbox();

    let ran = sandbox.run(&["scan", "app", "visual*", "--toml"]);

    ran.assert_status(ExitStatus::Success);
    assert!(
        ran.stdout.contains(&format!("path = \"{VS_CODE}\"")),
        "{}",
        ran.stdout
    );
}

#[test]
fn scan_app_finds_a_bundle_file_name_in_the_app_folders() {
    let sandbox = code_sandbox();

    let ran = sandbox.run(&["scan", "app", "visual studio code.app", "--toml"]);

    ran.assert_status(ExitStatus::Success);
    assert!(
        ran.stdout.contains(&format!("path = \"{VS_CODE}\"")),
        "{}",
        ran.stdout
    );
}

#[test]
fn scan_app_never_matches_the_bundle_name() {
    let sandbox = code_sandbox();
    sandbox.install_app("/Applications/Sublime.app", "com.sublime.Text", "Editor");

    let ran = sandbox.run(&["scan", "app", "Editor"]);

    ran.assert_status(ExitStatus::Failed);
    assert!(
        ran.stderr.contains("no app matches Editor"),
        "{}",
        ran.stderr
    );
}

#[test]
fn scan_app_compares_a_word_ending_in_app_against_the_full_file_name() {
    let sandbox = code_sandbox();
    sandbox.install_app(
        "/Applications/CodeWrangler.app",
        "com.wrangler.Code",
        "CodeWrangler",
    );

    let ran = sandbox.run_with(
        Script::terminal().pick(0),
        &["scan", "app", "Code.app", "--toml"],
    );

    ran.assert_status(ExitStatus::Success);
    assert_eq!(ran.asked, [format!("Choose an app: {VS_CODE} | {XCODE}")]);
}

#[test]
fn a_word_ending_in_app_in_place_of_a_mode_enters_app_mode() {
    let sandbox = code_sandbox();

    let ran = sandbox.run(&["scan", "visual studio code.app", "--toml"]);

    ran.assert_status(ExitStatus::Success);
    assert!(
        ran.stdout.contains(&format!("path = \"{VS_CODE}\"")),
        "{}",
        ran.stdout
    );
}

#[test]
fn an_app_word_in_place_of_a_mode_ignores_case() {
    let sandbox = code_sandbox();

    let scanned = sandbox.run(&["scan", "XCODE.APP", "--toml"]);
    let cleaned = sandbox.run_with(Script::terminal().answer("n"), &["clean", "Xcode.APP"]);

    scanned.assert_status(ExitStatus::Success);
    assert!(
        scanned.stdout.contains(&format!("path = \"{XCODE}\"")),
        "{}",
        scanned.stdout
    );
    cleaned.assert_status(ExitStatus::Success);
    assert!(cleaned.stdout.contains("Xcode.app"), "{}", cleaned.stdout);
}

#[test]
fn an_app_word_in_place_of_a_command_cleans_the_app() {
    let sandbox = code_sandbox();

    let ran = sandbox.run_with(Script::terminal().answer("n"), &["Xcode.app"]);

    ran.assert_status(ExitStatus::Success);
    assert!(ran.stdout.contains("app/com.apple.dt.Xcode"), "{}", ran.stdout);
}

#[test]
fn an_app_path_with_a_trailing_slash_enters_app_mode() {
    let sandbox = code_sandbox();

    let ran = sandbox.run(&["scan", "/Applications/Xcode.app/", "--toml"]);

    ran.assert_status(ExitStatus::Success);
    assert!(
        ran.stdout.contains(&format!("path = \"{XCODE}\"")),
        "{}",
        ran.stdout
    );
}

#[test]
fn an_app_path_in_place_of_a_mode_enters_app_mode() {
    let downloaded = "/Users/me/Downloads/Some.app";
    let sandbox = Sandbox::with_roots(&["/Users/me/Downloads", "/Users/me/Library"]);
    sandbox.install_app(downloaded, "com.some.Some", "Some");

    let ran = sandbox.run(&["scan", downloaded, "--toml"]);

    ran.assert_status(ExitStatus::Success);
    assert!(
        ran.stdout.contains(&format!("path = \"{downloaded}\"")),
        "{}",
        ran.stdout
    );
}

#[test]
fn app_mode_by_an_app_word_refuses_the_rule_flags() {
    let sandbox = code_sandbox();

    let ran = sandbox.run(&["scan", "Xcode.app", "--only", "node"]);

    ran.assert_status(ExitStatus::Usage);
    assert!(
        ran.stderr.contains("app mode does not take --only"),
        "{}",
        ran.stderr
    );
    assert!(ran.stderr.contains("  drop --only"), "{}", ran.stderr);
}

#[test]
fn scan_app_offers_only_the_matches_when_several_match() {
    let sandbox = code_sandbox();
    sandbox.install_app(BAR, "com.foo.Bar", "Bar");

    let ran = sandbox.run_with(
        Script::terminal().pick(1),
        &["scan", "app", "*code", "--toml"],
    );

    ran.assert_status(ExitStatus::Success);
    assert_eq!(ran.asked, [format!("Choose an app: {VS_CODE} | {XCODE}")]);
    assert!(
        ran.stdout.contains(&format!("path = \"{XCODE}\"")),
        "{}",
        ran.stdout
    );
}

#[test]
fn several_matches_without_a_terminal_are_a_usage_error_naming_each() {
    let sandbox = code_sandbox();

    let ran = sandbox.run_with(Script::piped(), &["scan", "app", "*code"]);

    ran.assert_status(ExitStatus::Usage);
    assert!(ran.asked.is_empty(), "{:?}", ran.asked);
    for line in [
        "*code matches 2 apps",
        "rosie scan app '/Applications/Visual Studio Code.app'",
        "rosie scan app /Applications/Xcode.app",
    ] {
        assert!(ran.stderr.contains(line), "{line}\n{}", ran.stderr);
    }
}

#[test]
fn scan_app_matching_nothing_says_so() {
    let sandbox = code_sandbox();

    let ran = sandbox.run(&["scan", "app", "Nope"]);

    ran.assert_status(ExitStatus::Failed);
    assert!(
        ran.stderr.contains("no app matches Nope"),
        "{}",
        ran.stderr
    );
}

#[test]
fn scan_app_takes_a_path_outside_the_app_folders() {
    let downloaded = "/Users/me/Downloads/Some.app";
    let sandbox = Sandbox::with_roots(&["/Users/me/Downloads", "/Users/me/Library"]);
    sandbox.install_app(downloaded, "com.some.Some", "Some");

    let ran = sandbox.run(&["scan", "app", downloaded, "--toml"]);

    ran.assert_status(ExitStatus::Success);
    assert!(
        ran.stdout.contains(&format!("path = \"{downloaded}\"")),
        "{}",
        ran.stdout
    );
}

#[test]
fn scan_app_takes_a_path_with_wildcards() {
    let sandbox = Sandbox::with_roots(&["/Users/me/Downloads", "/Users/me/Library"]);
    sandbox.install_app("/Users/me/Downloads/Some.app", "com.some.Some", "Some");
    sandbox.install_app("/Users/me/Downloads/Other.app", "com.other.Other", "Other");
    sandbox.fake.add_dir("/Users/me/Downloads/Sources");

    let one = sandbox.run(&["scan", "app", "/Users/me/Downloads/s*", "--toml"]);
    let several = sandbox.run_with(Script::piped(), &["scan", "/Users/me/Downloads/*.app"]);
    let none = sandbox.run(&["scan", "app", "/Users/me/Downloads/x*.app"]);

    one.assert_status(ExitStatus::Success);
    assert!(
        one.stdout
            .contains("path = \"/Users/me/Downloads/Some.app\""),
        "{}",
        one.stdout
    );
    several.assert_status(ExitStatus::Usage);
    assert!(
        several.stderr.contains("matches 2 apps"),
        "{}",
        several.stderr
    );
    none.assert_status(ExitStatus::Failed);
    assert!(
        none.stderr
            .contains("no app matches /Users/me/Downloads/x*.app"),
        "{}",
        none.stderr
    );
}

#[test]
fn scan_app_outside_the_roots_hints_at_adding_its_folder() {
    let downloaded = "/Users/me/Downloads/Some.app";
    let sandbox = Sandbox::with_roots(&["/Applications", "/Users/me/Library"]);
    sandbox.install_app(downloaded, "com.some.Some", "Some");

    let ran = sandbox.run(&["scan", "app", downloaded]);

    ran.assert_status(ExitStatus::Failed);
    for line in [
        "to allow just it:\n    rosie roots add /Users/me/Downloads/Some.app",
        "or to allow its folder:\n    rosie roots add /Users/me/Downloads",
    ] {
        assert!(ran.stderr.contains(line), "{line}\n{}", ran.stderr);
    }
}

#[test]
fn clean_app_deletes_the_app_and_its_leftovers_once_confirmed() {
    let sandbox = Sandbox::with_roots(&["/Applications", "/Users/me/Library"]);
    install_bar(&sandbox);

    let ran = sandbox.run_with(Script::terminal().answer("y"), &["clean", "app", BAR]);

    ran.assert_status(ExitStatus::Success);
    assert!(!sandbox.exists(BAR));
    assert!(!sandbox.exists("/Users/me/Library/Caches/com.foo.Bar"));
}

#[test]
fn scan_orphans_lists_leftovers_of_apps_no_longer_installed_as_skip() {
    let sandbox = Sandbox::with_roots(&["/Users/me/Library"]);
    sandbox
        .fake
        .add_dir("/Users/me/Library/Caches/com.gone.Tool");

    let ran = sandbox.run(&["scan", "orphans", "--toml"]);

    ran.assert_status(ExitStatus::Success);
    assert!(
        ran.stdout
            .contains("path = \"/Users/me/Library/Caches/com.gone.Tool\""),
        "{}",
        ran.stdout
    );
    assert!(
        ran.stderr.contains("skip (aggressive): 1 item"),
        "{}",
        ran.stderr
    );
}
