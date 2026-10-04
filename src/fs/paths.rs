//! Rosie's paths: the user's home folder, injected by `main` and never read from `$HOME`
//! below it, rosie's own folders under it, and the one call that makes any written path
//! absolute (`docs/spec/safety.md#symlinks`).

use std::path::{Path, PathBuf};

use super::Error;
use super::backend::Backend;
use super::gate::{Bounds, Gate, resolve_dots};

/// The home folder rosie acts for: absolute, with `.` and `..` resolved by text. Every
/// path the scans compare with home, or build under it, is spelled the same way, so a
/// home typed as `/Users/me/../me` answers exactly as `/Users/me` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    home: PathBuf,
}

impl Paths {
    /// Refuses a relative home: rosie never guesses a base.
    pub fn new(home: &Path) -> Result<Paths, Error> {
        let home = resolve_dots(home)?;
        Ok(Paths { home })
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Rosie's config folder, `~/.config/rosie`.
    pub fn config_dir(&self) -> PathBuf {
        self.home.join(".config/rosie")
    }

    /// Rosie's data folder, `~/.local/share/rosie`.
    pub fn data_dir(&self) -> PathBuf {
        self.home.join(".local/share/rosie")
    }

    /// What a gate acting for this home's user is bounded by: rosie's own folders
    /// under the home, the cleanup `roots`, and the user's uid.
    pub fn bounds(&self, roots: Vec<PathBuf>, user_uid: u32) -> Bounds {
        Bounds {
            roots,
            config_dir: self.config_dir(),
            data_dir: self.data_dir(),
            user_uid,
        }
    }

    /// Makes a written path absolute and accepts it only as its own canonical form on
    /// disk: [`Paths::expand`], then [`Gate::check_real`].
    pub fn resolve<B: Backend>(&self, gate: &Gate<B>, path: &Path) -> Result<PathBuf, Error> {
        let expanded = self.expand(path)?;
        gate.check_real(&expanded)
    }

    /// Makes a written path absolute by text alone: a leading `~` component is the home
    /// folder, and `.`, `..`, and repeated `/` are resolved. Any other relative path,
    /// `~user` included, is refused.
    pub fn expand(&self, path: &Path) -> Result<PathBuf, Error> {
        let anchored = match path.strip_prefix("~") {
            Ok(rest) => self.home.join(rest),
            Err(_) => path.to_path_buf(),
        };
        resolve_dots(&anchored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::fake::FakeBackend;

    const HOME: &str = "/Users/me";

    fn paths() -> Paths {
        Paths::new(Path::new(HOME)).expect("absolute home")
    }

    fn gate(fake: &FakeBackend) -> Gate<&FakeBackend> {
        Gate::new(fake, paths().bounds(Vec::new(), 501)).expect("absolute bounds")
    }

    #[test]
    fn expands_home_and_resolves_dots_and_repeated_slashes() {
        let cases = [
            ("~", "/Users/me"),
            ("~/", "/Users/me"),
            ("~//Library/Caches/x", "/Users/me/Library/Caches/x"),
            ("~/a/./b/../c", "/Users/me/a/c"),
            ("/Library//Caches/./x", "/Library/Caches/x"),
        ];

        for (written, expected) in cases {
            let expanded = paths().expand(Path::new(written));

            assert_eq!(
                expanded.expect("absolute once expanded"),
                PathBuf::from(expected),
                "{written}"
            );
        }
    }

    #[test]
    fn refuses_relative_paths_and_other_users_homes() {
        for written in ["relative/x", "~user/x", ""] {
            let expanded = paths().expand(Path::new(written));

            assert!(
                matches!(expanded, Err(Error::Relative { .. })),
                "{written}: {expanded:?}"
            );
        }
    }

    #[test]
    fn resolves_a_home_path_that_is_its_own_canonical_form() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/Library/Caches");

        let resolved = paths().resolve(&gate(&fake), Path::new("~//Library/Caches/gone"));

        assert_eq!(
            resolved.expect("real path"),
            PathBuf::from("/Users/me/Library/Caches/gone")
        );
    }

    #[test]
    fn refuses_a_missing_path_below_a_symlinked_ancestor() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/dotfiles/cargo");
        fake.add_symlink("/Users/me/.cargo", "dotfiles/cargo");

        let resolved = paths().resolve(&gate(&fake), Path::new("~/.cargo/registry"));

        let Err(Error::Symlink { link, real, .. }) = resolved else {
            panic!("expected a symlink refusal, got {resolved:?}");
        };
        assert_eq!(link, PathBuf::from("/Users/me/.cargo"));
        assert!(
            matches!(real, crate::fs::RealPath::Resolved(real) if real == Path::new("/Users/me/dotfiles/cargo/registry"))
        );
    }

    #[test]
    fn refuses_a_misspelling_naming_the_spelling_on_disk() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/Library/Caches");

        let resolved = paths().resolve(&gate(&fake), Path::new("~/library/caches/x"));

        let Err(Error::Misspelled { real, .. }) = resolved else {
            panic!("expected a spelling refusal, got {resolved:?}");
        };
        assert_eq!(real, PathBuf::from("/Users/me/Library/Caches/x"));
    }
}
