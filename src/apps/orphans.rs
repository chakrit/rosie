//! `rosie clean orphans`: leftovers of apps no longer installed
//! (`docs/spec/app.md#rosie-clean-orphans`).

use super::bundle::{installed_apps, no_id_report};
use super::locations::leftover_locations;
use super::matching::{mentions_id, reads_as_bundle_id};
use super::search::Search;
use super::{Error, Scanned};
use crate::fs::{Backend, Gate, Home};
use crate::plan::{AggressiveItems, Twin};
use crate::scan::Mounts;

const RULE: &str = "orphans";

/// Scans the leftover locations for entries named by the bundle ID of an app that is not
/// installed. Every orphan is aggressive.
///
/// An installed app rosie cannot identify fails the scan: its leftovers would otherwise
/// be listed as orphans. An installed app with no bundle-ID key cannot be matched to its
/// leftovers by ID either, so any it has may appear among the orphans; it is reported
/// only as a note, not a reason to fail the scan.
pub fn scan_orphans<B>(
    gate: &Gate<B>,
    home: &Home,
    aggressive: AggressiveItems,
    mounts: Mounts,
) -> Result<Scanned, Error>
where
    B: Backend + Sync,
{
    let installed = installed_apps(gate, home)?;
    if let Some(unidentified) = installed.unidentified.into_iter().next() {
        return Err(unidentified.into_error());
    }

    let mut search = Search::new(gate, home, mounts, aggressive);
    for path in &installed.no_id {
        search.add_report(no_id_report(path)?);
    }
    for location in leftover_locations(home) {
        for candidate in search.candidates(&location)? {
            let reads_as_id = reads_as_bundle_id(&candidate.name, location.holds);
            let accounted = installed
                .apps
                .iter()
                .any(|app| mentions_id(&app.id, &candidate.name));
            if reads_as_id && !accounted {
                search.add(candidate, location.holds, RULE, Twin::Aggressive)?;
            }
        }
    }
    search.finish()
}
