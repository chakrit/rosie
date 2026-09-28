//! Where a pack is pulled from: a GitHub `owner/repo` (`docs/spec/rules.md#packs`).

use std::fmt;
use std::str::FromStr;

use super::error::Error;
use super::name;

const DEFAULT_OWNER: &str = "chakrit";
const DEFAULT_REPO: &str = "rosie";
/// The pack name of the user-rules layer (`docs/spec/rules.md#layers`). A pack name the
/// volume folds onto it is refused at install, where the volume can be asked.
pub(super) const RESERVED_PACK: &str = "user";

/// A GitHub repository holding a pack in its `rules/` folder. Both names are valid pack
/// names, so each is one plain path component, and the pack is never named `user`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Source {
    owner: String,
    repo: String,
}

impl Source {
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The pack name, which is the repository name.
    pub fn pack(&self) -> &str {
        &self.repo
    }

    /// The repository's default-branch tarball.
    pub fn tarball_url(&self) -> String {
        format!(
            "https://github.com/{}/{}/archive/HEAD.tar.gz",
            self.owner, self.repo
        )
    }

    pub(super) fn from_names(owner: &str, repo: &str) -> Option<Source> {
        match name::is_valid(owner) && name::is_valid(repo) && repo != RESERVED_PACK {
            true => Some(Source {
                owner: owner.to_owned(),
                repo: repo.to_owned(),
            }),
            false => None,
        }
    }
}

/// The CLI's default source, github.com/chakrit/rosie.
impl Default for Source {
    fn default() -> Self {
        Source {
            owner: DEFAULT_OWNER.to_owned(),
            repo: DEFAULT_REPO.to_owned(),
        }
    }
}

impl FromStr for Source {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        let bad_source = || Error::BadSource {
            text: text.to_owned(),
        };

        let (owner, repo) = text.split_once('/').ok_or_else(bad_source)?;
        if repo == RESERVED_PACK {
            return Err(Error::ReservedPack {
                text: text.to_owned(),
            });
        }
        Source::from_names(owner, repo).ok_or_else(bad_source)
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owner_and_repo() {
        let source: Source = "someone/their-rules".parse().expect("valid source");

        assert_eq!(source.owner(), "someone");
        assert_eq!(source.pack(), "their-rules");
    }

    #[test]
    fn refuses_text_that_is_not_two_valid_names() {
        for text in [
            "rosie", "a/b/c", "/rosie", "chakrit/", "../x", "a/..", "a/.git", "a b/c",
        ] {
            let parsed = text.parse::<Source>();
            assert!(
                matches!(parsed, Err(Error::BadSource { .. })),
                "{text:?} should be refused, got {parsed:?}"
            );
        }
    }

    /// `docs/spec/rules.md#layers` names the user-rules layer's pack `user`. Other
    /// spellings are refused at install, by the volume (`tests/packs.rs`).
    #[test]
    fn refuses_a_pack_named_user() {
        let parsed = "someone/user".parse::<Source>();

        assert!(
            matches!(parsed, Err(Error::ReservedPack { .. })),
            "got {parsed:?}"
        );
    }

    #[test]
    fn accepts_an_owner_named_user() {
        let parsed = "user/their-rules".parse::<Source>();

        assert!(parsed.is_ok(), "got {parsed:?}");
    }
}
