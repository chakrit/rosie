//! Wildcard matching of one path component. A [`Glob`] matches exactly: rule markers
//! (`docs/spec/rules.md#folder-strategies`) and the components of a Lua `glob` pattern. A
//! [`Pattern`] is a command-line pattern, case folded: `--only`, the `app` argument, and
//! `tree <dir>` (`docs/spec/cli.md#patterns`).
//!
//! `*` matches any run of characters and `?` exactly one; everything else matches itself,
//! case-sensitively in a [`Glob`], ignoring case in a [`Pattern`]. In a glob, brackets,
//! braces, and backslashes are refused rather than taken literally, so a pattern written
//! for a richer glob syntax fails loudly instead of never matching. A plain [`Pattern`]
//! word holds no glob, so they are its literal characters.

use std::fmt;

use thiserror::Error;

/// Characters with glob meaning elsewhere that this syntax does not support.
const UNSUPPORTED: [char; 5] = ['[', ']', '{', '}', '\\'];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob(Box<str>);

impl Glob {
    pub fn parse(pattern: &str) -> Result<Glob, Error> {
        if pattern.is_empty() {
            return Err(Error::Empty);
        }
        if pattern.contains('/') {
            return Err(Error::Separator(pattern.into()));
        }
        if let Some(bad) = pattern.chars().find(|c| UNSUPPORTED.contains(c)) {
            return Err(Error::Unsupported(pattern.into(), bad));
        }

        Ok(Glob(pattern.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn matches(&self, name: &str) -> bool {
        let pattern: Vec<char> = self.0.chars().collect();
        let name: Vec<char> = name.chars().collect();
        match_chars(&pattern, &name)
    }
}

/// How one wildcard component of a path pattern matches the names in a folder: exactly,
/// as [`Glob`] does for rule files, or folded, as [`Pattern`] does for the
/// command line.
pub trait NameMatch: Sized {
    fn parse(text: &str) -> Result<Self, Error>;

    fn matches(&self, name: &str) -> bool;
}

impl NameMatch for Glob {
    fn parse(text: &str) -> Result<Glob, Error> {
        Glob::parse(text)
    }

    fn matches(&self, name: &str) -> bool {
        Glob::matches(self, name)
    }
}

/// A name pattern as the user types one. Case is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    text: Box<str>,
    matcher: Matcher,
}

/// How a pattern matches, each holding its text case-folded.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Matcher {
    /// A word without `*` or `?`: its characters, anywhere in the name.
    Anywhere(Box<str>),
    /// A word with `*` or `?`: a glob over the whole name.
    Whole(Glob),
}

impl Pattern {
    pub fn parse(text: &str) -> Result<Pattern, Error> {
        if text.is_empty() {
            return Err(Error::Empty);
        }

        let folded = text.to_lowercase();
        let matcher = match is_glob(text.as_bytes()) {
            true => Matcher::Whole(Glob::parse(&folded)?),
            false => Matcher::Anywhere(folded.into()),
        };
        Ok(Pattern {
            text: text.into(),
            matcher,
        })
    }

    pub fn matches(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        match &self.matcher {
            Matcher::Anywhere(word) => name.contains(&**word),
            Matcher::Whole(glob) => glob.matches(&name),
        }
    }
}

impl NameMatch for Pattern {
    fn parse(text: &str) -> Result<Pattern, Error> {
        Pattern::parse(text)
    }

    fn matches(&self, name: &str) -> bool {
        Pattern::matches(self, name)
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Whether `text` holds a `*` or `?`, which make a pattern a glob rather than a literal
/// name.
pub fn is_glob(text: &[u8]) -> bool {
    text.iter().any(|byte| matches!(byte, b'*' | b'?'))
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    #[error("a glob pattern cannot be empty")]
    Empty,

    #[error("glob pattern {0:?} names one file or folder and cannot contain `/`")]
    Separator(Box<str>),

    #[error("glob pattern {0:?} uses {1:?}; only `*` and `?` are supported")]
    Unsupported(Box<str>, char),
}

/// Iterative wildcard matching: on a mismatch, retry from the most recent `*` with it
/// swallowing one more character. Linear in practice, never exponential.
fn match_chars(pattern: &[char], name: &[char]) -> bool {
    let (mut p, mut n) = (0, 0);
    let mut last_star: Option<(usize, usize)> = None;

    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                last_star = Some((p, n));
                p += 1;
            }
            Some(&c) if c == '?' || c == name[n] => {
                p += 1;
                n += 1;
            }
            _ => {
                let Some((star, swallowed)) = last_star else {
                    return false;
                };
                p = star + 1;
                n = swallowed + 1;
                last_star = Some((star, swallowed + 1));
            }
        }
    }

    pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glob(pattern: &str) -> Glob {
        Glob::parse(pattern).expect("valid pattern")
    }

    #[test]
    fn matches_literal_names_exactly() {
        assert!(glob("Cargo.toml").matches("Cargo.toml"));
        assert!(!glob("Cargo.toml").matches("cargo.toml"));
        assert!(!glob("Cargo.toml").matches("Cargo.toml.bak"));
        assert!(!glob("Cargo.toml").matches("Cargo.tom"));
    }

    #[test]
    fn star_matches_any_run_including_none() {
        let tf = glob("*.tf");

        assert!(tf.matches("main.tf"));
        assert!(tf.matches(".tf"));
        assert!(!tf.matches("main.tfvars"));
        assert!(!tf.matches("main.tf.json"));
        assert!(glob("*").matches(""));
        assert!(glob("a*b*c").matches("aXXbYYbc"));
        assert!(!glob("a*b*c").matches("aXXcYYb"));
    }

    #[test]
    fn question_mark_matches_exactly_one_character() {
        assert!(glob("?.cfg").matches("é.cfg"));
        assert!(!glob("?.cfg").matches(".cfg"));
        assert!(!glob("?.cfg").matches("ab.cfg"));
    }

    #[test]
    fn refuses_patterns_outside_the_syntax() {
        assert_eq!(Glob::parse(""), Err(Error::Empty));
        assert!(matches!(Glob::parse("src/*.rs"), Err(Error::Separator(_))));
        assert!(matches!(
            Glob::parse("*.{js,ts}"),
            Err(Error::Unsupported(_, '{'))
        ));
        assert!(matches!(
            Glob::parse("[ab].txt"),
            Err(Error::Unsupported(_, '['))
        ));
    }

    fn pattern(text: &str) -> Pattern {
        Pattern::parse(text).expect("a valid pattern")
    }

    #[test]
    fn a_plain_word_matches_anywhere_in_the_name() {
        assert!(pattern("code").matches("CodeWrangler"));
        assert!(pattern("code").matches("Xcode"));
        assert!(pattern("code").matches("Visual Studio Code"));
        assert!(pattern("node").matches("node-modules"));
        assert!(!pattern("code").matches("Slack"));
    }

    #[test]
    fn a_plain_word_matches_its_characters_literally() {
        assert!(pattern("Foo [Beta]").matches("Foo [Beta] Pro"));
        assert!(pattern("a{b}").matches("xa{b}x"));
        assert!(!pattern("Foo [Beta]").matches("Foo B"));
    }

    #[test]
    fn a_wildcard_word_is_a_glob_over_the_whole_name() {
        assert!(pattern("code*").matches("CodeWrangler"));
        assert!(!pattern("code*").matches("Xcode"));
        assert!(pattern("*code").matches("Xcode"));
        assert!(!pattern("*code").matches("CodeWrangler"));
        assert!(pattern("?code").matches("Xcode"));
        assert!(!pattern("?code").matches("Code"));
    }

    #[test]
    fn ignores_case_beyond_ascii() {
        assert!(pattern("PyCache").matches("pycache"));
        assert!(pattern("éclair").matches("Éclair"));
        assert!(pattern("É*").matches("éclair"));
    }

    #[test]
    fn shows_the_text_as_typed() {
        assert_eq!(pattern("Code*").to_string(), "Code*");
    }

    #[test]
    fn refuses_what_a_glob_refuses() {
        assert_eq!(Pattern::parse(""), Err(Error::Empty));
        assert!(matches!(
            Pattern::parse("[ab]*"),
            Err(Error::Unsupported(_, '['))
        ));
    }
}
