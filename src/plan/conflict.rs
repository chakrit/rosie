//! Rule conflicts: entries whose rules disagree on tier, which only the user may resolve
//! (`docs/spec/plan.md#rule-conflicts`).
//!
//! No rule's tier wins over another's. A scan marks the entry `conflict`; a run and the
//! shell export refuse a plan that still holds one; the user picks ticked or unticked.

use std::collections::BTreeMap;
use std::fmt;
use std::ptr;

use super::{Delete, Plan, Selection, Status, Twin};

/// The rules that matched one entry, or entries nested in it, at each tier. Both lists
/// are non-empty and sorted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disagreement {
    normal: Vec<String>,
    aggressive: Vec<String>,
}

impl Disagreement {
    pub fn normal(&self) -> &[String] {
        &self.normal
    }

    pub fn aggressive(&self) -> &[String] {
        &self.aggressive
    }
}

/// `normal: rosie/node-modules; aggressive: alice/js`.
impl fmt::Display for Disagreement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "normal: {}; aggressive: {}",
            self.normal.join(", "),
            self.aggressive.join(", ")
        )
    }
}

/// What a conflict entry shows: `conflict, rules disagree (normal: a; aggressive: b).
/// Set status to ticked or unticked`.
pub(super) fn conflict_hint(disagreement: &Disagreement) -> String {
    format!("conflict, rules disagree ({disagreement}). Set status to ticked or unticked")
}

/// Each rule that matched a path, or a path nested in it, with its tier. One rule never
/// disagrees with itself: when its twins both match one path it counts as normal, and
/// when it also matches inside a path it matched, the outer match's tier stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Tiers(BTreeMap<String, Twin>);

/// What the rules of one entry say about its tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Verdict {
    Agreed(Twin),
    Disagreed(Disagreement),
}

impl Tiers {
    pub(super) fn of(rule: String, twin: Twin) -> Self {
        Tiers(BTreeMap::from([(rule, twin)]))
    }

    pub(super) fn add(&mut self, rule: String, twin: Twin) {
        let tier = self.0.entry(rule).or_insert(twin);
        if twin == Twin::Normal {
            *tier = Twin::Normal;
        }
    }

    pub(super) fn absorb(&mut self, inner: Tiers) {
        for (rule, twin) in inner.0 {
            self.0.entry(rule).or_insert(twin);
        }
    }

    pub(super) fn verdict(&self) -> Verdict {
        let (normal, aggressive): (Vec<_>, Vec<_>) =
            self.0.iter().partition(|(_, twin)| **twin == Twin::Normal);
        let names = |rules: Vec<(&String, &Twin)>| -> Vec<String> {
            rules.into_iter().map(|(rule, _)| rule.clone()).collect()
        };

        match (normal.is_empty(), aggressive.is_empty()) {
            (false, false) => Verdict::Disagreed(Disagreement {
                normal: names(normal),
                aggressive: names(aggressive),
            }),
            (true, false) => Verdict::Agreed(Twin::Aggressive),
            // `Tiers` is never empty: it starts from one rule.
            (false, true) | (true, true) => Verdict::Agreed(Twin::Normal),
        }
    }
}

impl Plan {
    /// The entries awaiting the user's choice, with what their rules say, in plan order.
    pub fn conflicts(&self) -> Vec<(&Delete, &Disagreement)> {
        self.deletes
            .iter()
            .filter_map(|delete| match &delete.status {
                Status::Conflict(disagreement) => Some((delete, disagreement)),
                Status::Ticked | Status::Unticked | Status::Blocked => None,
            })
            .collect()
    }

    /// The plan with each conflict in `choices` set to the chosen status. Entries are
    /// taken from [`Plan::conflicts`] and told apart by identity, not by value; any other
    /// entry stays as it is.
    pub fn resolving(&self, choices: &[(&Delete, Selection)]) -> Plan {
        let chosen = |delete: &Delete| {
            choices
                .iter()
                .find(|(conflict, _)| ptr::eq(*conflict, delete))
                .map(|(_, selection)| Status::from(*selection))
        };
        let deletes = self.deletes.restatusing(|delete| match chosen(delete) {
            Some(status) => status,
            None => delete.status.clone(),
        });

        Plan {
            deletes,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_at_one_tier_agree() {
        let mut tiers = Tiers::of("rosie/a".to_owned(), Twin::Aggressive);
        tiers.add("alice/b".to_owned(), Twin::Aggressive);

        assert_eq!(tiers.verdict(), Verdict::Agreed(Twin::Aggressive));
    }

    #[test]
    fn rules_at_different_tiers_disagree_naming_each_side() {
        let mut tiers = Tiers::of("rosie/a".to_owned(), Twin::Normal);
        let mut inner = Tiers::of("alice/b".to_owned(), Twin::Aggressive);
        inner.add("bob/c".to_owned(), Twin::Normal);
        tiers.absorb(inner);

        let Verdict::Disagreed(disagreement) = tiers.verdict() else {
            panic!("expected a disagreement, got {:?}", tiers.verdict());
        };
        assert_eq!(disagreement.normal(), ["bob/c", "rosie/a"]);
        assert_eq!(disagreement.aggressive(), ["alice/b"]);
    }

    #[test]
    fn one_rule_matching_inside_its_own_outer_match_keeps_the_outer_tier() {
        let mut outer = Tiers::of("rosie/a".to_owned(), Twin::Aggressive);
        outer.absorb(Tiers::of("rosie/a".to_owned(), Twin::Normal));

        assert_eq!(outer.verdict(), Verdict::Agreed(Twin::Aggressive));
    }

    #[test]
    fn one_rule_matching_at_both_tiers_is_normal() {
        let mut tiers = Tiers::of("rosie/a".to_owned(), Twin::Aggressive);
        tiers.add("rosie/a".to_owned(), Twin::Normal);
        tiers.add("rosie/a".to_owned(), Twin::Aggressive);

        assert_eq!(tiers.verdict(), Verdict::Agreed(Twin::Normal));
    }
}
