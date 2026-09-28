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

use std::path::{Path, PathBuf};

use crate::fs::{Argv, Backend, Gate};
use crate::plan::{Plan, WalkSkips};

pub use app::scan_app;
pub use error::Error;
pub use orphans::scan_orphans;

/// What an app or orphan scan found, and what it could not look into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scanned {
    pub plan: Plan,
    pub skips: WalkSkips,
}

/// The installed apps the `app` picker offers: `.app` bundles in `/Applications` and
/// `~/Applications` (`docs/spec/cli.md#pickers`), sorted. A folder rosie may not list
/// fails the listing, which would otherwise be incomplete.
pub fn picker_apps<B: Backend>(gate: &Gate<B>, home: &Path) -> Result<Vec<PathBuf>, Error> {
    let home = &crate::fs::resolve_dots(home)?;
    let folders = [PathBuf::from("/Applications"), home.join("Applications")];
    let found = bundle::bundles_in(gate, &folders)?;
    match found.unlistable.into_iter().next() {
        Some(unlistable) => Err(unlistable.cause.into()),
        None => Ok(found.bundles),
    }
}

/// Runs a read-only system command and returns its standard output.
fn run_for_text<B: Backend>(gate: &Gate<B>, argv: Argv) -> Result<String, Error> {
    let output = gate.run(&argv)?;
    if !output.exit.success() {
        return Err(Error::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit: output.exit,
            argv,
        });
    }
    String::from_utf8(output.stdout).map_err(|_| Error::NotUtf8 { argv })
}

#[cfg(test)]
mod fixture;
#[cfg(test)]
mod tests;
