//! Sandbox tier: the management commands, their pickers, and the app modes.

mod cli_fixture;
mod pack_fixture;
mod tarball;

use cli_fixture::{CONFIG_FILE, CWD, HOME, PACK, Sandbox, Script};
use pack_fixture::{Canned, url_of};
use rosie::cli::ExitStatus;
use rosie::fs::Argv;
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
fn rules_remove_without_a_pack_offers_the_pulled_packs() {
    let sandbox = Sandbox::new();

    let ran = sandbox.run_with(Script::terminal().pick(0), &["rules", "remove"]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(ran.asked, ["Choose a pack: rosie (chakrit/rosie)"]);
    assert!(!sandbox.exists(PACK));
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
            "roots =\n  {HOME}\nwalk.enter_bundles = false\nwalk.enter_mounts = false\nwalk.enter_placeholders = false\n"
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
    let script = Script::terminal().pick(1).pick(0);

    let ran = sandbox.run_with(script, &["config", "set"]);

    ran.assert_status(ExitStatus::Success);
    assert_eq!(
        ran.asked,
        [
            "Choose a key: walk.enter_bundles | walk.enter_mounts | walk.enter_placeholders",
            "Choose a value: true | false",
        ]
    );
    let got = sandbox.run(&["config", "get", "walk.enter_mounts"]);
    assert_eq!(got.stdout, "true\n");
}

#[test]
fn config_get_without_a_key_offers_every_key() {
    let sandbox = Sandbox::new();

    let ran = sandbox.run_with(Script::terminal().pick(0), &["config", "get"]);

    ran.assert_status(ExitStatus::Success);
    assert!(ran.asked[0].starts_with("Choose a key: roots | walk.enter_bundles"));
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
    let plist = format!("{BAR}/Contents/Info.plist");
    sandbox.fake.add_file(&plist, "<plist/>");
    sandbox
        .fake
        .add_sized_file(format!("{BAR}/Contents/MacOS/Bar"), 1 << 20);
    sandbox.respond(extract(&plist, "CFBundleIdentifier"), "com.foo.Bar\n");
    sandbox.respond(extract(&plist, "CFBundleName"), "Bar\n");
    sandbox.respond(Argv::new("/usr/sbin/pkgutil").arg("--pkgs"), "");
    sandbox.fake.add_dir("/Users/me/Library/Caches/com.foo.Bar");
}

fn extract(plist: &str, key: &str) -> Argv {
    Argv::new("/usr/bin/plutil")
        .arg("-extract")
        .arg(key)
        .arg("raw")
        .arg("-o")
        .arg("-")
        .arg(plist)
}

#[test]
fn scan_app_without_an_app_offers_the_installed_apps() {
    let sandbox = Sandbox::with_roots(&["/Applications", "/Users/me/Library"]);
    install_bar(&sandbox);

    let ran = sandbox.run_with(Script::terminal().pick(0), &["scan", "app"]);

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
fn scan_orphans_lists_leftovers_of_apps_no_longer_installed_unticked() {
    let sandbox = Sandbox::with_roots(&["/Users/me/Library"]);
    sandbox
        .fake
        .add_dir("/Users/me/Library/Caches/com.gone.Tool");

    let ran = sandbox.run(&["scan", "orphans"]);

    ran.assert_status(ExitStatus::Success);
    assert!(
        ran.stdout
            .contains("path = \"/Users/me/Library/Caches/com.gone.Tool\""),
        "{}",
        ran.stdout
    );
    assert!(
        ran.stderr
            .contains("unticked (aggressive, not run): 1 item"),
        "{}",
        ran.stderr
    );
}
