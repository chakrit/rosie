//! `[walk] enter_links`: how far the symlink ban is relaxed
//! (`docs/spec/safety.md#enter_links`).

use serde::Deserialize;

/// A level of `enter_links`. Each level allows what the one before it does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnterLinks {
    /// The ban applies in full.
    #[default]
    None,
    /// Symlink components are allowed in `roots` entries, rule fixed paths, and typed
    /// paths; the walk never follows a link.
    Roots,
    /// As `Roots`, and the walk follows a link whose target is inside the roots.
    All,
}

impl EnterLinks {
    pub const ALL: [EnterLinks; 3] = [EnterLinks::None, EnterLinks::Roots, EnterLinks::All];

    pub fn label(self) -> &'static str {
        match self {
            EnterLinks::None => "none",
            EnterLinks::Roots => "roots",
            EnterLinks::All => "all",
        }
    }

    /// The level a `config.toml` value or `--enter-links` names.
    pub fn parse(label: &str) -> Option<EnterLinks> {
        EnterLinks::ALL
            .into_iter()
            .find(|level| level.label() == label)
    }

    /// Whether a path rosie is given may pass through a symlink.
    pub fn allows_given_links(self) -> bool {
        match self {
            EnterLinks::None => false,
            EnterLinks::Roots | EnterLinks::All => true,
        }
    }

    /// Whether the walk follows a link.
    pub fn follows_in_walk(self) -> bool {
        match self {
            EnterLinks::None | EnterLinks::Roots => false,
            EnterLinks::All => true,
        }
    }
}
