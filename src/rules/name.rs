//! Pack and rule names, the `pack/rule` identity (`docs/spec/rules.md#names`), and the
//! patterns `--only` selects rules by (`docs/spec/cli.md#patterns`).

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

use super::error::Error;
use crate::wildcard;

/// The longest pack or rule name, in characters.
pub(super) const MAX_CHARS: usize = 64;

/// A valid rule name, compared as written.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(Box<str>);

impl Name {
    /// Accepts an ASCII letter first, then ASCII letters, digits, `_`, or `-`, at most 64
    /// characters in all.
    pub fn parse(text: &str) -> Result<Name, Error> {
        valid(text).map(Name)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A valid pack name, or a pack source's owner name. It is spelled as written, and
/// compares, orders, and hashes ignoring ASCII case, as GitHub's names do
/// (`docs/spec/rules.md#packs`).
#[derive(Debug, Clone)]
pub struct PackName(Box<str>);

impl PackName {
    /// Accepts what [`Name::parse`] accepts.
    pub fn parse(text: &str) -> Result<PackName, Error> {
        valid(text).map(PackName)
    }

    /// The pack the user's own rules belong to.
    pub fn user() -> PackName {
        PackName(USER_PACK.into())
    }

    pub fn is_user(&self) -> bool {
        *self == PackName::user()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn folded(&self) -> impl Iterator<Item = u8> + '_ {
        self.0.bytes().map(|byte| byte.to_ascii_lowercase())
    }
}

const USER_PACK: &str = "user";

impl PartialEq for PackName {
    fn eq(&self, other: &PackName) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl Eq for PackName {}

impl Ord for PackName {
    fn cmp(&self, other: &PackName) -> Ordering {
        self.folded().cmp(other.folded())
    }
}

impl PartialOrd for PackName {
    fn partial_cmp(&self, other: &PackName) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Hash for PackName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.folded().for_each(|byte| state.write_u8(byte));
        state.write_usize(self.0.len());
    }
}

impl fmt::Display for PackName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The text of a valid pack or rule name (`docs/spec/rules.md#names`).
fn valid(text: &str) -> Result<Box<str>, Error> {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return Err(Error::EmptyName);
    };
    if !first.is_ascii_alphabetic() {
        return Err(Error::BadFirst(first));
    }
    if let Some(bad) = chars.find(|&c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
        return Err(Error::BadChar(bad));
    }

    let length = text.len();
    match length > MAX_CHARS {
        true => Err(Error::TooLong(length)),
        false => Ok(text.into()),
    }
}

/// A rule's identity: the pack it belongs to and its name within the pack.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleId {
    pub pack: PackName,
    pub rule: Name,
}

impl RuleId {
    /// Parses a qualified `pack/rule` name, as an override names the rule it replaces.
    pub fn parse_qualified(text: &str) -> Result<RuleId, Error> {
        let Some((pack, rule)) = text.split_once('/') else {
            return Err(Error::Unqualified);
        };
        let pack = PackName::parse(pack)?;
        let rule = Name::parse(rule)?;
        Ok(RuleId { pack, rule })
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.pack, self.rule)
    }
}

/// A `rule` or `pack/rule` pattern, each side a command-line [`wildcard::Pattern`]. A bare rule
/// pattern matches in any pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    text: Box<str>,
    pack: wildcard::Pattern,
    rule: wildcard::Pattern,
}

impl Pattern {
    pub fn parse(text: &str) -> Result<Pattern, Error> {
        let (pack, rule) = text.split_once('/').unwrap_or(("*", text));
        if rule.contains('/') {
            return Err(Error::PatternShape);
        }

        Ok(Pattern {
            text: text.into(),
            pack: wildcard::Pattern::parse(pack)?,
            rule: wildcard::Pattern::parse(rule)?,
        })
    }

    pub fn matches(&self, id: &RuleId) -> bool {
        self.rule.matches(id.rule.as_str()) && self.pack.matches(id.pack.as_str())
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ascii_letters_digits_underscores_and_dashes() {
        for text in ["node-modules", "cargo_target", "py3", "Rosie", "x"] {
            assert_eq!(Name::parse(text).expect("a valid name").to_string(), text);
        }
    }

    #[test]
    fn refuses_a_first_character_that_is_not_an_ascii_letter() {
        for (text, first) in [
            ("-x", '-'),
            ("_x", '_'),
            (".x", '.'),
            (" x", ' '),
            ("9lives", '9'),
            ("ñandú", 'ñ'),
        ] {
            assert!(
                matches!(Name::parse(text), Err(Error::BadFirst(found)) if found == first),
                "{text}"
            );
        }
    }

    #[test]
    fn refuses_anything_but_ascii_letters_digits_underscores_and_dashes() {
        for (text, bad) in [
            ("a/b", '/'),
            ("a.b", '.'),
            ("a b", ' '),
            ("a\tb", '\t'),
            ("a!", '!'),
            ("a+b", '+'),
            ("a·b", '·'),
            ("café", 'é'),
            ("a🦀", '🦀'),
        ] {
            assert!(
                matches!(Name::parse(text), Err(Error::BadChar(found)) if found == bad),
                "{text}"
            );
        }
        assert!(matches!(Name::parse(""), Err(Error::EmptyName)));
    }

    #[test]
    fn refuses_more_than_64_characters() {
        let longest = "a".repeat(64);
        let too_long = "a".repeat(65);

        assert!(Name::parse(&longest).is_ok());
        assert!(matches!(Name::parse(&too_long), Err(Error::TooLong(65))));
    }

    #[test]
    fn parses_a_qualified_name() {
        let id = RuleId::parse_qualified("rosie/docker").expect("qualified");

        assert_eq!((id.pack.as_str(), id.rule.as_str()), ("rosie", "docker"));
        assert_eq!(id.to_string(), "rosie/docker");
        assert!(matches!(
            RuleId::parse_qualified("docker"),
            Err(Error::Unqualified)
        ));
        assert!(matches!(
            RuleId::parse_qualified("rosie/a/b"),
            Err(Error::BadChar('/'))
        ));
        assert!(matches!(
            RuleId::parse_qualified("my pack/x"),
            Err(Error::BadChar(' '))
        ));
    }

    fn pattern(text: &str) -> Pattern {
        Pattern::parse(text).expect("valid pattern")
    }

    fn id(text: &str) -> RuleId {
        RuleId::parse_qualified(text).expect("qualified")
    }

    #[test]
    fn a_bare_pattern_matches_the_rule_name_in_any_pack() {
        assert!(pattern("pycache").matches(&id("rosie/pycache")));
        assert!(pattern("pycache").matches(&id("extra/pycache")));
        assert!(pattern("py*").matches(&id("extra/pycache")));
        assert!(pattern("go-buil?").matches(&id("rosie/go-build")));
        assert!(!pattern("py*").matches(&id("python/cache")));
    }

    #[test]
    fn a_plain_word_matches_anywhere_in_the_name() {
        assert!(pattern("node").matches(&id("rosie/node-modules")));
        assert!(pattern("cache").matches(&id("rosie/pycache")));
        assert!(pattern("ros/cache").matches(&id("rosie/pycache")));
        assert!(!pattern("node").matches(&id("rosie/pycache")));
        assert!(!pattern("node/cache").matches(&id("rosie/pycache")));
    }

    #[test]
    fn a_qualified_pattern_matches_its_pack_side_and_rule_side() {
        assert!(pattern("rosie/pycache").matches(&id("rosie/pycache")));
        assert!(!pattern("rosie/pycache").matches(&id("extra/pycache")));
        assert!(pattern("rosie/*").matches(&id("rosie/go-build")));
        assert!(!pattern("rosie/*").matches(&id("extra/go-build")));
        assert!(pattern("*/go-build").matches(&id("extra/go-build")));
        assert!(pattern("ex*/py*").matches(&id("extra/pycache")));
        assert!(!pattern("ex*/py*").matches(&id("extra/go-build")));
    }

    #[test]
    fn both_sides_ignore_case() {
        assert!(pattern("Rosie/pycache").matches(&id("rosie/pycache")));
        assert!(pattern("ro*/pycache").matches(&id("Rosie/pycache")));
        assert!(pattern("PyCache").matches(&id("rosie/pycache")));
        assert!(pattern("rosie/Py*").matches(&id("rosie/pycache")));
    }

    #[test]
    fn refuses_a_malformed_pattern() {
        assert!(matches!(Pattern::parse("a/b/c"), Err(Error::PatternShape)));
        assert!(matches!(
            Pattern::parse(""),
            Err(Error::Pattern(wildcard::Error::Empty))
        ));
        assert!(matches!(
            Pattern::parse("rosie/"),
            Err(Error::Pattern(wildcard::Error::Empty))
        ));
        assert!(matches!(
            Pattern::parse("py[c]*"),
            Err(Error::Pattern(wildcard::Error::Unsupported(..)))
        ));
    }
}
