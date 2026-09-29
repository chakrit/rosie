//! Where a pack is pulled from: a GitHub `owner/repo` (`docs/spec/rules.md#packs`).

use std::fmt;
use std::str::FromStr;

use super::error::Error;
use crate::rules::Name;

const DEFAULT_OWNER: &str = "chakrit";
const DEFAULT_REPO: &str = "rosie";

/// A GitHub repository holding a pack in its `rules/` folder. Both names are valid pack
/// names, so each is one plain path component, and the pack is never named `user`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Source {
    owner: Name,
    repo: Name,
}

impl Source {
    pub fn owner(&self) -> &str {
        self.owner.as_str()
    }

    /// The pack name, which is the repository name.
    pub fn pack(&self) -> &Name {
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
        let owner = Name::parse(owner).ok()?;
        let repo = Name::parse(repo).ok()?;
        Source::from_valid_names(owner, repo).ok()
    }

    /// A source from names already parsed, refused when the pack name is the reserved
    /// `user`.
    pub(super) fn from_valid_names(owner: Name, repo: Name) -> Result<Source, ReservedPackName> {
        match repo.is_user_pack() {
            true => Err(ReservedPackName),
            false => Ok(Source { owner, repo }),
        }
    }
}

/// A pack name that is the reserved `user`, which only the user's own rules carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReservedPackName;

/// The CLI's default source, github.com/chakrit/rosie.
impl Default for Source {
    fn default() -> Self {
        Source {
            owner: Name::parse(DEFAULT_OWNER).expect("the default owner is a valid name"),
            repo: Name::parse(DEFAULT_REPO).expect("the default repo is a valid name"),
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
        if repo == Name::user_pack().as_str() {
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
    fn the_default_source_builds() {
        let source = Source::default();

        assert_eq!(source.to_string(), "chakrit/rosie");
    }

    #[test]
    fn parses_owner_and_repo() {
        let source: Source = "someone/their-rules".parse().expect("valid source");

        assert_eq!(source.owner(), "someone");
        assert_eq!(source.pack().as_str(), "their-rules");
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

    /// `docs/spec/rules.md#layers-and-overrides` names the user-rules layer's pack `user`. Other
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

    /// Folder names read back from the volume reach `from_names` without passing
    /// `from_str`, so it refuses the reserved pack name itself.
    #[test]
    fn names_read_back_never_make_a_pack_named_user() {
        assert_eq!(Source::from_names("someone", "user"), None);
        assert!(Source::from_names("someone", "users").is_some());
    }
}
