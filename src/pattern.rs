//! Command-line patterns: `--only`, the `app` argument, and `tree <dir>`
//! (`docs/spec/cli.md#patterns`).

use std::fmt;

use crate::glob::{self, Glob, NameMatch, has_wildcard};

/// A name pattern as the user types one. A word without `*` or `?` matches anywhere in
/// the name; a word with them is a glob over the whole name. Case is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    text: Box<str>,
    glob: Glob,
}

impl Pattern {
    pub fn parse(text: &str) -> Result<Pattern, glob::Error> {
        let typed = Glob::parse(&text.to_lowercase())?;
        let glob = match has_wildcard(text.as_bytes()) {
            true => typed,
            false => typed.anywhere(),
        };

        Ok(Pattern {
            text: text.into(),
            glob,
        })
    }

    pub fn matches(&self, name: &str) -> bool {
        self.glob.matches(&name.to_lowercase())
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
            Pattern::parse("[ab]"),
            Err(glob::Error::Unsupported(_, '['))
        ));
    }
}
