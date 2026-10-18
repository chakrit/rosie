//! Rule conflicts: entries whose rules disagree on tier, which only the user may resolve
//! (`docs/spec/plan.md#rule-conflicts`).
//!
//! No rule's tier wins over another's. A scan marks the entry `conflict`; a run and the
//! shell export refuse a plan that still holds one; the user picks `clean` or `skip`.

use std::collections::BTreeMap;
use std::fmt;
use std::ptr;

use super::{Delete, Plan, Selection, Status};
use crate::tier::Tier;

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
/// Set status to clean or skip`.
pub(super) fn conflict_hint(disagreement: &Disagreement) -> String {
    format!("conflict, rules disagree ({disagreement}). Set status to clean or skip")
}

/// Each rule that matched a path, or a path nested in it, with its tier. One rule never
/// disagrees with itself: when its twins both match one path it counts as normal, and
/// when it also matches inside a path it matched, the outer match's tier stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Tiers(BTreeMap<String, Tier>);

/// What the rules of one entry say about its tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Verdict {
    Agreed(Tier),
    Disagreed(Disagreement),
}

impl Tiers {
    pub(super) fn of(rule: String, tier: Tier) -> Self {
        Tiers(BTreeMap::from([(rule, tier)]))
    }

    pub(super) fn add(&mut self, rule: String, tier: Tier) {
        let stored = self.0.entry(rule).or_insert(tier);
        if tier == Tier::Normal {
            *stored = Tier::Normal;
        }
    }

    pub(super) fn absorb(&mut self, inner: Tiers) {
        for (rule, tier) in inner.0 {
            self.0.entry(rule).or_insert(tier);
        }
    }

    pub(super) fn verdict(&self) -> Verdict {
        let (normal, aggressive): (Vec<_>, Vec<_>) =
            self.0.iter().partition(|(_, tier)| **tier == Tier::Normal);
        let names = |rules: Vec<(&String, &Tier)>| -> Vec<String> {
            rules.into_iter().map(|(rule, _)| rule.clone()).collect()
        };

        match (normal.is_empty(), aggressive.is_empty()) {
            (false, false) => Verdict::Disagreed(Disagreement {
                normal: names(normal),
                aggressive: names(aggressive),
            }),
            (true, false) => Verdict::Agreed(Tier::Aggressive),
            // `Tiers` is never empty: it starts from one rule.
            (false, true) | (true, true) => Verdict::Agreed(Tier::Normal),
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
                Status::Clean | Status::Skip(_) => None,
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
                .map(|(_, selection)| selection.answered())
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
        let mut tiers = Tiers::of("rosie/a".to_owned(), Tier::Aggressive);
        tiers.add("alice/b".to_owned(), Tier::Aggressive);

        assert_eq!(tiers.verdict(), Verdict::Agreed(Tier::Aggressive));
    }

    #[test]
    fn rules_at_different_tiers_disagree_naming_each_side() {
        let mut tiers = Tiers::of("rosie/a".to_owned(), Tier::Normal);
        let mut inner = Tiers::of("alice/b".to_owned(), Tier::Aggressive);
        inner.add("bob/c".to_owned(), Tier::Normal);
        tiers.absorb(inner);

        let Verdict::Disagreed(disagreement) = tiers.verdict() else {
            panic!("expected a disagreement, got {:?}", tiers.verdict());
        };
        assert_eq!(disagreement.normal(), ["bob/c", "rosie/a"]);
        assert_eq!(disagreement.aggressive(), ["alice/b"]);
    }

    #[test]
    fn one_rule_matching_inside_its_own_outer_match_keeps_the_outer_tier() {
        let mut outer = Tiers::of("rosie/a".to_owned(), Tier::Aggressive);
        outer.absorb(Tiers::of("rosie/a".to_owned(), Tier::Normal));

        assert_eq!(outer.verdict(), Verdict::Agreed(Tier::Aggressive));
    }

    #[test]
    fn one_rule_matching_at_both_tiers_is_normal() {
        let mut tiers = Tiers::of("rosie/a".to_owned(), Tier::Aggressive);
        tiers.add("rosie/a".to_owned(), Tier::Normal);
        tiers.add("rosie/a".to_owned(), Tier::Aggressive);

        assert_eq!(tiers.verdict(), Verdict::Agreed(Tier::Normal));
    }
}
