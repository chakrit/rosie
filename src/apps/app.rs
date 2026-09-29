//! `rosie clean app <X.app>`: the bundle and the files it leaves behind
//! (`docs/spec/app.md#rosie-clean-app-xapp`).

use std::ffi::OsString;
use std::path::Path;

use super::bundle::{
    Bundle, BundleId, Unidentified, Unlistable, installed_apps, is_app_path, read_bundle,
};
use super::listing::{Listing, list_folder};
use super::locations::{Holds, leftover_locations};
use super::matching::{claimed_by_other, claims_by_id, claims_by_name};
use super::search::{Candidate, Search};
use super::{Error, Scanned};
use crate::fs::{Backend, FileKind, Gate, Home, SystemArgv, SystemTool};
use crate::plan::{AggressiveItems, Report, Twin};
use crate::process::{self, ProcessTable};
use crate::scan::{Entry, Mounts};

/// Scans for `app` and its leftovers in the user's `home` and the system folders.
///
/// The app argument is a user-typed path: a symlink or misspelling in it is refused, and
/// so is a bundle that is the root of a mounted volume, since app mode never crosses
/// volumes. The plan is refused while any process executes from the bundle. An installed app rosie
/// cannot identify is reported, since leftovers the two apps share may be ticked.
///
/// The app itself may have no bundle-ID key: it is the app the user asked to remove, not
/// one rosie must first identify among others, so this is not refused. Its own bundle is
/// still ticked and its `CFBundleName` still matches leftovers by name (aggressively,
/// like any name-only match); it simply has no bundle ID to match leftovers by.
pub fn scan_app<B>(
    gate: &Gate<B>,
    app: &Path,
    home: &Home,
    aggressive: AggressiveItems,
    mounts: Mounts,
) -> Result<Scanned, Error>
where
    B: Backend + Sync,
{
    let path = gate.check_typed_path(app)?;
    let entry = Entry::lstat(gate, &path)?;
    if entry.is_mount(gate)? {
        return Err(Error::Mount { path });
    }
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into();
    let itself = match (entry.meta().kind, is_app_path(&path)) {
        (FileKind::Dir, true) => Candidate::of(entry, name),
        _ => None,
    };
    let Some(itself) = itself else {
        return Err(Error::NotAnApp { path });
    };
    let bundle = read_bundle(gate, &path)?;
    refuse_running(gate, &bundle)?;
    let installed = installed_apps(gate, home)?;
    let others: Vec<BundleId> = installed
        .apps
        .into_iter()
        .filter(|other| other.path != bundle.path)
        .map(|other| other.id)
        .collect();
    let rule = match &bundle.id {
        Some(id) => format!("app/{}", id.as_str()),
        None => "app".to_owned(),
    };

    let mut search = Search::new(gate, home, mounts, aggressive);
    search.add(itself, Holds::Plain, &rule, Twin::Normal)?;

    for location in leftover_locations(home) {
        for candidate in search.candidates(&location)? {
            let id_twin = bundle
                .id
                .as_ref()
                .filter(|id| claims_by_id(id, &candidate.name, location.holds))
                .map(|id| id_match_twin(id, &others, &candidate.name, location.holds));
            let by_name = bundle
                .name
                .as_ref()
                .is_some_and(|name| claims_by_name(name, &candidate.name));
            match (id_twin, by_name) {
                (Some(twin), _) => search.add(candidate, location.holds, &rule, twin)?,
                (None, true) => search.add(candidate, location.holds, &rule, Twin::Aggressive)?,
                (None, false) => {}
            }
        }
    }

    for (package, twin) in receipts(gate, &bundle, &others)? {
        search.add_receipt(&package, twin)?;
    }
    for report in reports(gate, &bundle)? {
        search.add_report(report);
    }
    for unidentified in &installed.unidentified {
        search.add_report(unidentified_report(unidentified, &bundle)?);
    }
    search.finish()
}

/// A bundle-ID match is ticked, unless it also belongs to another installed app, `others`:
/// a second copy with the same ID, or an app with a longer ID that also matches it
/// (`com.foo.Bar.canary.plist` while `com.foo.Bar.canary` is installed and `com.foo.Bar`
/// is being removed). Then it is aggressive.
fn id_match_twin(id: &BundleId, others: &[BundleId], name: &str, holds: Holds) -> Twin {
    let shared = others
        .iter()
        .any(|other| claimed_by_other(id, other, name, holds));
    match shared {
        true => Twin::Aggressive,
        false => Twin::Normal,
    }
}

fn refuse_running<B: Backend>(gate: &Gate<B>, bundle: &Bundle) -> Result<(), Error> {
    let processes = ProcessTable::query(gate)?;
    match processes.executing_from(&bundle.path) {
        Some(process) => Err(Error::Running {
            app: bundle.path.clone(),
            pid: process.pid,
        }),
        None => Ok(()),
    }
}

// receipts

pub(super) fn pkgs_argv() -> SystemArgv {
    SystemTool::PKGUTIL.argv().arg("--pkgs")
}

/// The installed package receipts whose IDs are bundle-ID matches of the app, each with
/// the twin [`id_match_twin`] gives it. An app with no bundle ID matches none.
fn receipts<B: Backend>(
    gate: &Gate<B>,
    bundle: &Bundle,
    others: &[BundleId],
) -> Result<Vec<(String, Twin)>, Error> {
    let Some(id) = &bundle.id else {
        return Ok(Vec::new());
    };
    let listed = process::run_tool_text(gate, &pkgs_argv())?;
    let matching = listed
        .lines()
        .filter(|package| claims_by_id(id, package, Holds::Plain))
        .map(|package| {
            let twin = id_match_twin(id, others, package, Holds::Plain);
            (package.to_owned(), twin)
        });
    Ok(matching.collect())
}

// reports

/// What a bundle ships that rosie cannot remove itself, reported with the steps the
/// user takes by hand.
#[derive(Debug, Clone, Copy)]
enum Manual {
    LoginItem,
    SystemExtension,
}

impl Manual {
    const ALL: [Manual; 2] = [Manual::LoginItem, Manual::SystemExtension];

    /// Where the bundle holds them.
    fn folder(self) -> &'static str {
        match self {
            Manual::LoginItem => "Contents/Library/LoginItems",
            Manual::SystemExtension => "Contents/Library/SystemExtensions",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Manual::LoginItem => "login item",
            Manual::SystemExtension => "system extension",
        }
    }

    fn steps(self, item: &str, app: &str) -> Vec<String> {
        match self {
            Manual::LoginItem => vec![
                "Open System Settings > General > Login Items & Extensions".to_owned(),
                format!("Remove {item} ({app}) from Open at Login and Allow in the Background"),
            ],
            Manual::SystemExtension => vec![
                format!("Before {app} is deleted, remove {item} from within {app}"),
                "Or open System Settings > General > Login Items & Extensions > Extensions \
                 and turn it off"
                    .to_owned(),
                "Check what remains with: systemextensionsctl list".to_owned(),
            ],
        }
    }
}

/// Login items and system extensions the bundle ships, as report-only entries.
fn reports<B: Backend>(gate: &Gate<B>, bundle: &Bundle) -> Result<Vec<Report>, Error> {
    let mut found = Vec::new();
    for manual in Manual::ALL {
        let names = match list_folder(gate, &bundle.path.join(manual.folder()))? {
            Listing::Entries(names) => names,
            Listing::NoFolder => continue,
            Listing::Denied(error) => return Err(error.into()),
        };
        found.extend(names.into_iter().map(|name| (manual, name)));
    }
    manual_reports(found, &bundle.display_name())
}

/// The reports for the login items and system extensions of `app` found in its bundle,
/// ordered by subject so the plan reads the same across runs, independent of the order
/// folders happen to list their entries in.
fn manual_reports(found: Vec<(Manual, OsString)>, app: &str) -> Result<Vec<Report>, Error> {
    let reports = found.into_iter().map(|(manual, name)| {
        let item = name.to_string_lossy();
        let subject = format!("{} {item} of {app}", manual.label());
        Report::new(subject, manual.steps(&item, app))
    });
    let mut reports = reports.collect::<Result<Vec<_>, _>>()?;

    reports.sort_by(|a, b| a.subject().cmp(b.subject()));
    Ok(reports)
}

/// An installed app rosie cannot identify, which may share leftovers with `bundle`.
fn unidentified_report(unidentified: &Unidentified, bundle: &Bundle) -> Result<Report, Error> {
    let path = unidentified.path().display();
    let (subject, other) = match unidentified {
        Unidentified::Bundle { cause, .. } => (
            format!("bundle ID of the installed app {path} cannot be read: {cause}"),
            "that app",
        ),
        Unidentified::Folder(Unlistable { cause, .. }) => (
            format!("app folder {path} cannot be listed: {cause}"),
            "the apps in that folder",
        ),
    };
    let app = bundle.display_name();
    let steps = vec![
        format!("Leftovers {app} shares with {other} are not told apart, so they may be ticked"),
        "Untick any item such an app still uses before running the plan".to_owned(),
    ];
    Ok(Report::new(subject, steps)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_ordered_by_subject_regardless_of_listing_order() {
        let found = vec![
            (Manual::SystemExtension, OsString::from("net.ext")),
            (Manual::LoginItem, OsString::from("Zed.app")),
            (Manual::LoginItem, OsString::from("Alpha.app")),
        ];

        let reports = manual_reports(found, "Bar").expect("subjects are non-empty");

        assert_eq!(
            reports.iter().map(Report::subject).collect::<Vec<_>>(),
            [
                "login item Alpha.app of Bar",
                "login item Zed.app of Bar",
                "system extension net.ext of Bar",
            ]
        );
    }

    /// The fake answers by this same argv, so only this test pins what `pkgutil` is asked.
    #[test]
    fn pkgutil_is_asked_for_every_package_id() {
        assert_eq!(pkgs_argv().to_string(), "/usr/sbin/pkgutil --pkgs");
    }
}
