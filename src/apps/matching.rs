//! Which leftover names belong to an app (`docs/spec/app.md`). Pure: names in, answers
//! out. Matches that claim a leftover are case-exact; only the orphan scan's exclusions
//! fold case (an installed app accounting for a leftover, and the `com.apple` domain),
//! and each can only keep a leftover out of the plan.

use super::bundle::{BundleId, BundleName, is_id_part};
use super::locations::Holds;

/// A bundle-ID match: the name equals the ID or starts with `<id>.`, and in Group
/// Containers also the team-prefixed `<team>.<id>` form.
pub(super) fn claims_by_id(id: &BundleId, name: &str, holds: Holds) -> bool {
    let plain = dot_bounded(id.as_str(), name);
    let team_prefixed = holds == Holds::GroupContainers
        && without_team(name).is_some_and(|rest| dot_bounded(id.as_str(), rest));
    plain || team_prefixed
}

/// A name-only match: the name equals the app's `CFBundleName` or starts with
/// `<name>.`. `BundleName` is never empty, so `dot_bounded` can never see an empty key
/// here and claim every name that starts with `.`.
pub(super) fn claims_by_name(app_name: &BundleName, name: &str) -> bool {
    dot_bounded(app_name.as_str(), name)
}

/// Whether a bundle-ID match of `id` also belongs to `other`, another installed app whose
/// ID equals `id` (a second copy) or extends it (`com.foo.Bar.canary` for `com.foo.Bar`).
/// Such matches are demoted to aggressive, since the other app still uses them.
pub(super) fn claimed_by_other(id: &BundleId, other: &BundleId, name: &str, holds: Holds) -> bool {
    let equal_or_longer = dot_bounded(id.as_str(), other.as_str());
    equal_or_longer && claims_by_id(other, name, holds)
}

/// Whether an installed app accounts for a leftover in orphan scans: its bundle ID
/// appears anywhere in the name as whole dot-separated parts, in any case, as in
/// `com.foo.Bar.plist`, `com.foo.bar.plist`, `<team>.com.foo.Bar`, or
/// `group.com.foo.Bar`. Wider than [`claims_by_id`], so a leftover in doubt is never
/// listed as an orphan.
pub(super) fn mentions_id(id: &BundleId, name: &str) -> bool {
    let id_parts: Vec<&str> = id.as_str().split('.').collect();
    let name_parts: Vec<&str> = name.split('.').collect();
    let same_parts = |window: &[&str]| {
        window
            .iter()
            .zip(&id_parts)
            .all(|(part, id_part)| part.eq_ignore_ascii_case(id_part))
    };
    name_parts.windows(id_parts.len()).any(same_parts)
}

/// Whether a leftover name reads as an app's bundle ID, so orphan scans consider it: at
/// least three well-formed dot-separated parts, the first all lowercase letters as a
/// reverse-DNS domain starts (`com`, `org`, `io`), after a Group Containers team
/// prefix. Anything naming Apple's `com.apple` domain is excluded.
pub(super) fn reads_as_bundle_id(name: &str, holds: Holds) -> bool {
    let id = match holds {
        Holds::GroupContainers => without_team(name).unwrap_or(name),
        Holds::Plain | Holds::LaunchAgents | Holds::LaunchDaemons => name,
    };
    let parts: Vec<&str> = id.split('.').collect();

    let domain_first = parts
        .first()
        .is_some_and(|first| first.bytes().all(|b| b.is_ascii_lowercase()));
    let shaped = parts.len() >= 3 && parts.iter().all(|part| is_id_part(part)) && domain_first;
    shaped && !names_apple(&parts)
}

/// `com.apple` anywhere, in any case, as in `com.apple.Safari` or
/// `group.com.apple.notes`.
fn names_apple(parts: &[&str]) -> bool {
    parts
        .windows(2)
        .any(|pair| pair[0].eq_ignore_ascii_case("com") && pair[1].eq_ignore_ascii_case("apple"))
}

fn dot_bounded(key: &str, name: &str) -> bool {
    name == key
        || name
            .strip_prefix(key)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// The rest of a Group Containers name after its team ID: ten uppercase letters or
/// digits, then a dot.
fn without_team(name: &str) -> Option<&str> {
    let (team, rest) = name.split_once('.')?;
    let is_team = team.len() == 10
        && team
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit());
    is_team.then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(text: &str) -> BundleId {
        BundleId::parse(text).expect("valid bundle id")
    }

    #[test]
    fn a_bundle_id_match_is_dot_bounded() {
        let bar = id("com.foo.Bar");
        let claims = |name| claims_by_id(&bar, name, Holds::Plain);

        assert!(claims("com.foo.Bar"));
        assert!(claims("com.foo.Bar.plist"));
        assert!(claims("com.foo.Bar.helper.savedState"));
        assert!(!claims("com.foo.Barista"));
        assert!(!claims("com.foo.Barista.plist"));
        assert!(!claims("com.foo.bar.plist"));
        assert!(!claims("xcom.foo.Bar"));
    }

    #[test]
    fn group_containers_also_match_the_team_prefixed_form() {
        let bar = id("com.foo.Bar");

        assert!(claims_by_id(
            &bar,
            "ABCDE12345.com.foo.Bar",
            Holds::GroupContainers
        ));
        assert!(claims_by_id(
            &bar,
            "ABCDE12345.com.foo.Bar.shared",
            Holds::GroupContainers
        ));
        assert!(claims_by_id(&bar, "com.foo.Bar", Holds::GroupContainers));
        assert!(!claims_by_id(
            &bar,
            "ABCDE12345.com.foo.Barista",
            Holds::GroupContainers
        ));
        assert!(!claims_by_id(
            &bar,
            "abcde12345.com.foo.Bar",
            Holds::GroupContainers
        ));
        assert!(!claims_by_id(
            &bar,
            "ABCDE1234.com.foo.Bar",
            Holds::GroupContainers
        ));
        assert!(!claims_by_id(
            &bar,
            "ABCDE123456.com.foo.Bar",
            Holds::GroupContainers
        ));
        assert!(!claims_by_id(&bar, "ABCDE12345.com.foo.Bar", Holds::Plain));
    }

    #[test]
    fn a_name_match_is_dot_bounded_and_exact() {
        let bar = BundleName::parse("Bar".to_owned()).expect("non-empty name");
        let claims = |name| claims_by_name(&bar, name);

        assert!(claims("Bar"));
        assert!(claims("Bar.log"));
        assert!(!claims("Barista"));
        assert!(!claims("bar"));
    }

    #[test]
    fn an_empty_bundle_name_cannot_be_constructed() {
        assert!(BundleName::parse(String::new()).is_none());
    }

    #[test]
    fn only_an_equal_or_longer_installed_id_takes_a_match_away() {
        let bar = id("com.foo.Bar");
        let taken = |other: &str, name| claimed_by_other(&bar, &id(other), name, Holds::Plain);

        assert!(taken("com.foo.Bar.canary", "com.foo.Bar.canary.plist"));
        assert!(!taken("com.foo.Bar.canary", "com.foo.Bar.plist"));
        assert!(!taken("com.foo.Bar.canary", "com.foo.Bar.canaryish"));
        assert!(taken("com.foo.Bar", "com.foo.Bar.plist"));
        assert!(taken("com.foo.Bar", "com.foo.Bar.canary.plist"));
        assert!(!taken("com.foo", "com.foo.Bar.plist"));
        assert!(!taken("com.foo.Barista", "com.foo.Barista.plist"));
    }

    #[test]
    fn an_installed_id_accounts_for_names_that_mention_it_whole() {
        let bar = id("com.foo.Bar");

        assert!(mentions_id(&bar, "com.foo.Bar.plist"));
        assert!(mentions_id(&bar, "group.com.foo.Bar"));
        assert!(mentions_id(&bar, "ABCDE12345.com.foo.Bar.shared"));
        assert!(!mentions_id(&bar, "com.foo.Barista.plist"));
        assert!(!mentions_id(&bar, "com.foo.Baz"));
        assert!(mentions_id(&bar, "com.foo.bar.plist"));
        assert!(mentions_id(&bar, "COM.FOO.BAR"));
    }

    #[test]
    fn orphan_candidates_read_as_non_apple_bundle_ids() {
        let reads = |name| reads_as_bundle_id(name, Holds::Plain);

        assert!(reads("com.foo.Bar"));
        assert!(reads("com.foo.Bar.plist"));
        assert!(!reads("Bar.log"));
        assert!(!reads("Slack"));
        assert!(!reads(".DS_Store"));
        assert!(!reads("Foo.Bar.baz"));
        assert!(!reads("com.apple.Safari"));
        assert!(!reads("com.Apple.Safari.plist"));
        assert!(!reads("group.com.apple.notes"));
        assert!(reads("com.applesauce.Foo"));
        assert!(reads("org.apple.Foo"));
        assert!(!reads("com.foo"));
        assert!(!reads("com.foo bar.baz"));
        assert!(!reads("group.COM.apple.notes"));
        assert!(reads_as_bundle_id(
            "ABCDE12345.com.foo.Bar",
            Holds::GroupContainers
        ));
        assert!(!reads_as_bundle_id(
            "ABCDE12345.com.apple.x",
            Holds::GroupContainers
        ));
    }
}
