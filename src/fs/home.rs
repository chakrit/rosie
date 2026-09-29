//! The user's home folder, injected by `main` and never read from `$HOME` below it.

use std::ops::Deref;
use std::path::{Path, PathBuf};

use super::Error;
use super::gate::resolve_dots;

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
