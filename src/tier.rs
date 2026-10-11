//! The two tiers of a rule's "what to clean" fields: the plain field and its
//! `_aggressive` twin (`docs/spec/rules.md#aggressive-twins`).

/// A "what to clean" value and its `_aggressive` twin; at least one is set
/// (`docs/spec/rules.md#aggressive-twins`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Twin<T> {
    Normal(T),
    Aggressive(T),
    Both { normal: T, aggressive: T },
}

impl<T> Twin<T> {
    pub(crate) fn from_parts(normal: Option<T>, aggressive: Option<T>) -> Option<Twin<T>> {
        match (normal, aggressive) {
            (Some(normal), Some(aggressive)) => Some(Twin::Both { normal, aggressive }),
            (Some(normal), None) => Some(Twin::Normal(normal)),
            (None, Some(aggressive)) => Some(Twin::Aggressive(aggressive)),
            (None, None) => None,
        }
    }

    pub fn normal(&self) -> Option<&T> {
        match self {
            Twin::Normal(normal) | Twin::Both { normal, .. } => Some(normal),
            Twin::Aggressive(_) => None,
        }
    }

    pub fn aggressive(&self) -> Option<&T> {
        match self {
            Twin::Aggressive(aggressive) | Twin::Both { aggressive, .. } => Some(aggressive),
            Twin::Normal(_) => None,
        }
    }

    /// Each value set, with the tier it belongs to.
    pub fn tiers(&self) -> impl Iterator<Item = (Tier, &T)> {
        let normal = self.normal().map(|value| (Tier::Normal, value));
        let aggressive = self.aggressive().map(|value| (Tier::Aggressive, value));
        normal.into_iter().chain(aggressive)
    }
}

/// Whether an item comes from a plain field or its `_aggressive` twin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Normal,
    Aggressive,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Normal => "normal",
            Tier::Aggressive => "aggressive",
        }
    }
}
