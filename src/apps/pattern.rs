//! Finding installed apps by a name pattern (`docs/spec/app.md#app-name-matching`).

use std::fmt;
use std::path::{Path, PathBuf};

use super::Error;
use super::bundle::{bundle_stem, names_an_app};
use crate::fs::{Backend, FsContext};
use crate::glob;
use crate::pattern::Pattern;

/// A command-line [`Pattern`] over app file names. One ending in `.app` is compared
/// against the full file name; any other against the file name without its final `.app`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPattern {
    pattern: Pattern,
    against: Against,
}

/// Which spelling of an app's file name a pattern is compared against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Against {
    /// `Visual Studio Code.app`.
    FileName,
    /// `Visual Studio Code`.
    Stem,
}

impl AppPattern {
    pub fn parse(text: &str) -> Result<AppPattern, glob::Error> {
        let against = match names_an_app(text) {
            true => Against::FileName,
            false => Against::Stem,
        };

        Ok(AppPattern {
            pattern: Pattern::parse(text)?,
            against,
        })
    }

    pub fn matches(&self, bundle: &Path) -> bool {
        let name = match self.against {
            Against::FileName => bundle.file_name().unwrap_or_default().to_string_lossy(),
            Against::Stem => bundle_stem(bundle).into(),
        };
        self.pattern.matches(&name)
    }
}

impl fmt::Display for AppPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.pattern.fmt(f)
    }
}

/// The apps the picker offers that `pattern` matches, sorted.
pub fn find<B: Backend>(ctx: &FsContext<B>, pattern: &AppPattern) -> Result<Vec<PathBuf>, Error> {
    let apps = super::picker_apps(ctx)?;
    Ok(apps
        .into_iter()
        .filter(|app| pattern.matches(app))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPS: [&str; 5] = [
        "/Applications/Visual Studio Code.app",
        "/Applications/Xcode.app",
        "/Applications/CodeWrangler.app",
        "/Applications/Code.app",
        "/Applications/myapp.app.co.th.app",
    ];

    /// The file names, without `.app`, of the apps `text` matches.
    fn matched(text: &str) -> Vec<String> {
        let pattern = AppPattern::parse(text).expect("a valid pattern");
        APPS.iter()
            .map(Path::new)
            .filter(|app| pattern.matches(app))
            .map(bundle_stem)
            .collect()
    }

    #[test]
    fn a_plain_word_matches_anywhere_in_the_name() {
        assert_eq!(
            matched("code"),
            ["Visual Studio Code", "Xcode", "CodeWrangler", "Code"]
        );
        assert_eq!(matched("app"), ["myapp.app.co.th"]);
        assert_eq!(matched("myapp.app"), ["myapp.app.co.th"]);
    }

    #[test]
    fn a_word_ending_in_app_is_compared_against_the_full_file_name() {
        assert_eq!(matched("Code.app"), ["Visual Studio Code", "Xcode", "Code"]);
        assert_eq!(matched("*.app").len(), APPS.len());
        assert!(matched("myapp.app?").is_empty());
    }

    #[test]
    fn a_wildcard_word_is_a_glob_over_the_name_without_app() {
        assert_eq!(matched("*code"), ["Visual Studio Code", "Xcode", "Code"]);
        assert_eq!(matched("code*"), ["CodeWrangler", "Code"]);
        assert_eq!(matched("?code"), ["Xcode"]);
        assert_eq!(matched("myapp.app*"), ["myapp.app.co.th"]);
    }

    #[test]
    fn ignores_case() {
        assert_eq!(matched("VISUAL*"), ["Visual Studio Code"]);
        assert_eq!(matched("XCODE.APP"), ["Xcode"]);
    }
}
