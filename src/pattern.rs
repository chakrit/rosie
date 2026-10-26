//! Command-line patterns: `--only`, the `app` argument, and `tree <dir>`
//! (`docs/spec/cli.md#patterns`).

use std::fmt;

use crate::glob::{self, Glob, NameMatch, has_wildcard};

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
    pub fn parse(text: &str) -> Result<Pattern, glob::Error> {
        if text.is_empty() {
            return Err(glob::Error::Empty);
        }

        let folded = text.to_lowercase();
        let matcher = match has_wildcard(text.as_bytes()) {
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
    fn parse(text: &str) -> Result<Pattern, glob::Error> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(Pattern::parse(""), Err(glob::Error::Empty));
        assert!(matches!(
            Pattern::parse("[ab]*"),
            Err(glob::Error::Unsupported(_, '['))
        ));
    }
}
