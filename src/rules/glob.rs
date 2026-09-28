//! Glob patterns for one path component: rule markers (`docs/spec/rules.md#folder-strategies`)
//! and the components of a Lua `glob` pattern.
//!
//! `*` matches any run of characters and `?` exactly one; everything else matches itself,
//! case-sensitively. Brackets, braces, and backslashes are refused rather than taken
//! literally, so a pattern written for a richer glob syntax fails loudly instead of never
//! matching.

use thiserror::Error;

/// Characters with glob meaning elsewhere that this syntax does not support.
const UNSUPPORTED: [char; 5] = ['[', ']', '{', '}', '\\'];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob(Box<str>);

impl Glob {
    pub fn parse(pattern: &str) -> Result<Glob, GlobError> {
        if pattern.is_empty() {
            return Err(GlobError::Empty);
        }
        if pattern.contains('/') {
            return Err(GlobError::Separator(pattern.into()));
        }
        if let Some(bad) = pattern.chars().find(|c| UNSUPPORTED.contains(c)) {
            return Err(GlobError::Unsupported(pattern.into(), bad));
        }
        Ok(Glob(pattern.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn matches(&self, name: &str) -> bool {
        let pattern: Vec<char> = self.0.chars().collect();
        let name: Vec<char> = name.chars().collect();
        wildcard_match(&pattern, &name)
    }
}

/// Iterative wildcard matching: on a mismatch, retry from the most recent `*` with it
/// swallowing one more character. Linear in practice, never exponential.
fn wildcard_match(pattern: &[char], name: &[char]) -> bool {
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

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GlobError {
    #[error("a glob pattern cannot be empty")]
    Empty,

    #[error("glob pattern {0:?} names one file or folder and cannot contain `/`")]
    Separator(Box<str>),

    #[error("glob pattern {0:?} uses {1:?}; only `*` and `?` are supported")]
    Unsupported(Box<str>, char),
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
        assert_eq!(Glob::parse(""), Err(GlobError::Empty));
        assert!(matches!(
            Glob::parse("src/*.rs"),
            Err(GlobError::Separator(_))
        ));
        assert!(matches!(
            Glob::parse("*.{js,ts}"),
            Err(GlobError::Unsupported(_, '{'))
        ));
        assert!(matches!(
            Glob::parse("[ab].txt"),
            Err(GlobError::Unsupported(_, '['))
        ));
    }
}
