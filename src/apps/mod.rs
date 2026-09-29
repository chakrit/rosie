//! `app` and `orphans` modes: search the app-leftover folders for bundle-ID and name
//! matches and list them as a plan (`docs/spec/app.md`). Scanning only; the runner
//! executes the plan. Every filesystem item is gated by `roots` through the same FS
//! layer as every mode.

mod app;
mod bundle;
mod error;
mod listing;
mod locations;
mod matching;
mod orphans;
mod search;

use std::path::PathBuf;

use crate::fs::{Backend, Gate, Home};
use crate::plan::{Plan, WalkSkips};
use crate::scan::Skipped;

pub use app::scan_app;
pub use error::Error;
pub use orphans::scan_orphans;

/// What an app or orphan scan found, and what it could not look into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scanned {
    pub plan: Plan,
    /// Paths the scan or the sizing did not enter, sorted by path.
    pub skipped: Vec<Skipped>,
}

impl Scanned {
    /// Walk-skip counts per reason, for the stats.
    pub fn skip_counts(&self) -> WalkSkips {
        self.skipped.iter().map(|skipped| skipped.reason).collect()
    }
}

/// The installed apps the `app` picker offers: `.app` bundles in `/Applications` and
/// `~/Applications` (`docs/spec/cli.md#pickers`), sorted. A folder rosie may not list
/// fails the listing, which would otherwise be incomplete.
pub fn picker_apps<B: Backend>(gate: &Gate<B>, home: &Home) -> Result<Vec<PathBuf>, Error> {
    let folders = [PathBuf::from("/Applications"), home.join("Applications")];
    let found = bundle::bundles_in(gate, &folders)?;
    match found.unlistable.into_iter().next() {
        Some(unlistable) => Err(unlistable.cause.into()),
        None => Ok(found.bundles),
    }
}

#[cfg(test)]
mod fixture;
#[cfg(test)]
mod tests;
