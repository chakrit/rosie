//! Pack and rule names, and the `pack/rule` identity (`docs/spec/rules.md#names`).

use std::fmt;

use thiserror::Error;

/// The longest pack or rule name, in characters.
const MAX_CHARS: usize = 64;

/// A valid pack or rule name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(Box<str>);

impl Name {
    /// Accepts an ASCII letter first, then ASCII letters, digits, `_`, or `-`, at most 64
    /// characters in all.
    pub fn parse(text: &str) -> Result<Name, NameError> {
        let mut chars = text.chars();
        let Some(first) = chars.next() else {
            return Err(NameError::Empty);
        };
        if !first.is_ascii_alphabetic() {
            return Err(NameError::BadFirst(first));
        }
        if let Some(bad) = chars.find(|&c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
            return Err(NameError::BadChar(bad));
        }

        let length = text.len();
        match length > MAX_CHARS {
            true => Err(NameError::TooLong(length)),
            false => Ok(Name(text.into())),
        }
    }

    /// The pack the user's own rules belong to.
    pub fn user_pack() -> Name {
        Name(USER_PACK.into())
    }

    pub fn is_user_pack(&self) -> bool {
        &*self.0 == USER_PACK
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

const USER_PACK: &str = "user";

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A rule's identity: the pack it belongs to and its name within the pack.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleId {
    pub pack: Name,
    pub rule: Name,
}

impl RuleId {
    /// Parses a qualified `pack/rule` name, as an override names the rule it replaces.
    pub fn parse_qualified(text: &str) -> Result<RuleId, NameError> {
        let Some((pack, rule)) = text.split_once('/') else {
            return Err(NameError::Unqualified);
        };
        let pack = Name::parse(pack)?;
        let rule = Name::parse(rule)?;
        Ok(RuleId { pack, rule })
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.pack, self.rule)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NameError {
    #[error("a name cannot be empty")]
    Empty,

    #[error("a name must start with an ASCII letter, not {0:?}")]
    BadFirst(char),

    #[error("a name may hold only ASCII letters, digits, `_`, and `-`, not {0:?}")]
    BadChar(char),

    #[error("a name is at most {MAX_CHARS} characters, not {0}")]
    TooLong(usize),

    #[error("a qualified name is `pack/rule`")]
    Unqualified,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ascii_letters_digits_underscores_and_dashes() {
        for text in ["node-modules", "cargo_target", "py3", "Rosie", "x"] {
            assert_eq!(
                Name::parse(text).map(|name| name.to_string()),
                Ok(text.to_string())
            );
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
            assert_eq!(Name::parse(text), Err(NameError::BadFirst(first)), "{text}");
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
            assert_eq!(Name::parse(text), Err(NameError::BadChar(bad)), "{text}");
        }
        assert_eq!(Name::parse(""), Err(NameError::Empty));
    }

    #[test]
    fn refuses_more_than_64_characters() {
        let longest = "a".repeat(64);
        let too_long = "a".repeat(65);

        assert!(Name::parse(&longest).is_ok());
        assert_eq!(Name::parse(&too_long), Err(NameError::TooLong(65)));
    }

    #[test]
    fn parses_a_qualified_name() {
        let id = RuleId::parse_qualified("rosie/docker").expect("qualified");

        assert_eq!((id.pack.as_str(), id.rule.as_str()), ("rosie", "docker"));
        assert_eq!(id.to_string(), "rosie/docker");
        assert_eq!(
            RuleId::parse_qualified("docker"),
            Err(NameError::Unqualified)
        );
        assert_eq!(
            RuleId::parse_qualified("rosie/a/b"),
            Err(NameError::BadChar('/'))
        );
        assert_eq!(
            RuleId::parse_qualified("my pack/x"),
            Err(NameError::BadChar(' '))
        );
    }
}
