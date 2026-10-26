//! Finding installed apps by a name pattern (`docs/spec/app.md#app-name-matching`).

use std::fmt;
use std::path::PathBuf;

use rayon::prelude::*;

use super::Error;
use super::bundle::{bundle_stem, read_name};
use crate::fs::{Backend, FsContext};
use crate::glob::{Glob, GlobError};

/// An app name with `*` and `?` wildcards, as `--only` takes, matched whole and ignoring
/// case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPattern {
    text: Box<str>,
    glob: Glob,
}

impl AppPattern {
    pub fn parse(text: &str) -> Result<AppPattern, GlobError> {
        Ok(AppPattern {
            text: text.into(),
            glob: Glob::parse(&text.to_lowercase())?,
        })
    }

    pub fn matches(&self, name: &str) -> bool {
        self.glob.matches(&name.to_lowercase())
    }
}

impl fmt::Display for AppPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// The apps the picker offers whose file name, without `.app`, or whose `CFBundleName`
/// `pattern` matches, sorted. Names are read only for bundles whose file name does not
/// match.
pub fn apps_matching<B>(ctx: &FsContext<B>, pattern: &AppPattern) -> Result<Vec<PathBuf>, Error>
where
    B: Backend + Sync,
{
    let (by_file, others): (Vec<PathBuf>, Vec<PathBuf>) = super::picker_apps(ctx)?
        .into_iter()
        .partition(|path| pattern.matches(&bundle_stem(path)));

    let named: Vec<Option<PathBuf>> = others
        .into_par_iter()
        .map(|path| {
            let name = read_name(ctx, &path)?;
            Ok(name
                .is_some_and(|name| pattern.matches(name.as_str()))
                .then_some(path))
        })
        .collect::<Result<_, Error>>()?;
    let by_name = named.into_iter().flatten();

    let mut found: Vec<PathBuf> = by_file.into_iter().chain(by_name).collect();
    found.sort();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(text: &str) -> AppPattern {
        AppPattern::parse(text).expect("a valid pattern")
    }

    #[test]
    fn matches_the_whole_name_ignoring_case() {
        assert!(pattern("code").matches("Code"));
        assert!(pattern("VISUAL STUDIO CODE").matches("Visual Studio Code"));
        assert!(!pattern("Code").matches("Visual Studio Code"));
        assert!(!pattern("Code").matches("Xcode"));
    }

    #[test]
    fn matches_wildcards_as_only_does() {
        assert!(pattern("*code").matches("Xcode"));
        assert!(pattern("visual*").matches("Visual Studio Code"));
        assert!(pattern("Sla?k").matches("Slack"));
        assert!(!pattern("Sla?k").matches("Slak"));
    }

    #[test]
    fn folds_case_beyond_ascii() {
        assert!(pattern("éclair").matches("Éclair"));
    }

    #[test]
    fn refuses_what_a_glob_refuses() {
        assert_eq!(AppPattern::parse(""), Err(GlobError::Empty));
        assert!(matches!(
            AppPattern::parse("[ab]"),
            Err(GlobError::Unsupported(_, '['))
        ));
    }
}
