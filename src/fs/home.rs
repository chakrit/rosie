//! The user's home folder, injected by `main` and never read from `$HOME` below it.

use std::ops::Deref;
use std::path::{Path, PathBuf};

use super::Error;
use super::gate::{Bounds, resolve_dots};

/// The user's home folder: absolute, with `.` and `..` resolved by text. Every path the
/// scans compare with home, or build under it, is spelled the same way, so a home typed
/// as `/Users/me/../me` answers exactly as `/Users/me` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home(PathBuf);

impl Home {
    /// Refuses a relative path: rosie never guesses a base.
    pub fn new(path: &Path) -> Result<Home, Error> {
        Ok(Home(resolve_dots(path)?))
    }

    /// Rosie's config folder, `~/.config/rosie`.
    pub fn config_dir(&self) -> PathBuf {
        self.0.join(".config/rosie")
    }

    /// Rosie's data folder, `~/.local/share/rosie`.
    pub fn data_dir(&self) -> PathBuf {
        self.0.join(".local/share/rosie")
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
}

impl Deref for Home {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Home {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}
