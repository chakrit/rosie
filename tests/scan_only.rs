//! Fixture tier for `--only`: a scan with a rule set narrowed to the named rules
//! (`docs/spec/cli.md#flags`).

mod scan_fixture;

use scan_fixture::*;

const RULES: &str = "[rules.node-modules]\nstrategy = \"marker\"\ntarget = \"node_modules\"\nmarker = [\"package.json\"]\n\
    [rules.vendor]\nstrategy = \"name\"\ntarget = \"vendor\"\n\
    [rules.npm-cache]\nstrategy = \"path\"\npaths = [\"~/.npm\"]\n\
    [rules.yarn-cache]\nstrategy = \"path\"\npaths = [\"~/.yarn\"]\n\
    [rules.brew]\nstrategy = \"tool\"\ncmd = \"brew cleanup\"\n";

fn home() -> rosie::fs::fake::FakeBackend {
    let fake = fake_home();
    add_rules(&fake, "rules.toml", RULES);
    fake.add_file("/Users/me/code/app/vendor/pkg/package.json", "{}");
    fake.add_file("/Users/me/code/app/vendor/pkg/node_modules/x.js", "x");
    fake.add_file("/Users/me/.npm/index", "x");
    fake.add_file("/Users/me/.yarn/index", "x");
    fake
}

fn only(names: &[&'static str]) -> Options {
    Options {
        only: names.to_vec(),
        ..Options::default()
    }
}

#[test]
fn an_unselected_outer_match_does_not_hide_a_selected_inner_match() {
    let fake = home();

    let every_rule = tree(&fake, CODE);
    let selected = tree_with(&fake, CODE, only(&["node-modules"]));

    assert_eq!(paths(&every_rule), ["/Users/me/code/app/vendor"]);
    assert_eq!(
        entries(&selected)
            .into_iter()
            .map(|(path, rules, _)| (path, rules))
            .collect::<Vec<_>>(),
        [(
            "/Users/me/code/app/vendor/pkg/node_modules",
            vec!["rosie/node-modules"]
        )]
    );
}

#[test]
fn caches_plans_only_the_selected_fixed_paths_and_tools() {
    let fake = home();

    let paths_only = caches_with(&fake, only(&["yarn-cache"]));
    let tools_only = caches_with(&fake, only(&["brew"]));

    assert_eq!(paths(&paths_only), ["/Users/me/.yarn"]);
    assert!(paths_only.plan.tools().is_empty());
    assert!(paths(&tools_only).is_empty());
    assert_eq!(tools_only.plan.tools().len(), 1);
}
