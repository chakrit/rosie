//! `app` and `orphans` modes: search the app-leftover folders for bundle-ID and name
//! matches and list them as a plan (`docs/spec/app.md`), in the same [`Scan`] every mode
//! returns. Scanning only; the runner executes the plan. Every filesystem item is gated
//! by `roots` through the same FS layer as every mode, and refused, as in every mode,
//! while a process executes from it.
//!
//! [`Scan`]: crate::scan::Scan

mod app;
mod bundle;
mod claims;
mod error;
mod listing;
mod locations;
mod matching;
mod orphans;
mod search;

use std::path::PathBuf;

use crate::fs::{Backend, FsContext};

pub use app::scan_app;
pub use bundle::is_app_path;
pub use claims::{launch_jobs_argv, system_jobs_argv};
pub use error::Error;
pub use orphans::scan_orphans;

/// The installed apps the `app` picker offers: `.app` bundles in `/Applications` and
/// `~/Applications` (`docs/spec/cli.md#pickers`), sorted. A folder rosie may not list
/// fails the listing, which would otherwise be incomplete.
pub fn picker_apps<B: Backend>(ctx: &FsContext<B>) -> Result<Vec<PathBuf>, Error> {
    let folders = [
        PathBuf::from("/Applications"),
        ctx.paths().home().join("Applications"),
    ];
    let found = bundle::bundles_in(ctx, &folders)?;
    match found.unlistable.into_iter().next() {
        Some(unlistable) => Err(unlistable.cause.into()),
        None => Ok(found.bundles),
    }
}

#[cfg(test)]
mod fixture;
#[cfg(test)]
mod tests;
