//! `rosie clean app <X.app>`: the bundle and the files it leaves behind
//! (`docs/spec/app.md#rosie-clean-app-xapp`).

use std::cell::OnceCell;
use std::ffi::OsString;
use std::path::Path;

use super::Error;
use super::bundle::{
    Bundle, BundleId, Installed, Unidentified, Unlistable, installed_apps, is_app_path, read_bundle,
};
use super::listing::{Listing, list_folder};
use super::locations::{Holds, leftover_locations};
use super::matching::{claimed_by_other, claims_by_id, claims_by_name};
use super::search::{Candidate, Search};
use crate::fs::{
    self, Backend, Cleanup, FileKind, FsContext, FsOp, SystemArgv, SystemTool, UserInput,
};
use crate::plan::Report;
use crate::scan::{Entry, Mounts, NotAnEntry, Scan, Settings};
use crate::tier::Tier;

/// Scans apps through one context. What the machine has installed, its apps and its
/// package receipts, is read on the first scan that needs it and reused by every later
/// scan, so `rosie check` with several apps reads it once.
pub struct AppScanner<'c, B: Backend> {
    ctx: &'c FsContext<B>,
    installed: OnceCell<Installed>,
    packages: OnceCell<String>,
}

impl<'c, B: Backend + Sync> AppScanner<'c, B> {
    pub fn new(ctx: &'c FsContext<B>) -> Self {
        AppScanner {
            ctx,
            installed: OnceCell::new(),
            packages: OnceCell::new(),
        }
    }

    /// Scans for `app` and its leftovers in the user's home and the system folders.
    ///
    /// The app argument is a user-typed path: a symlink or misspelling in it is refused, and
    /// so is a bundle that is a dataless placeholder, which rosie does not open, or the root
    /// of a mounted volume, since rosie never plans a mount point, which would plan the
    /// whole volume. A filesystem or `plutil` failure reading the bundle itself is
    /// [`Error::UnreadableBundle`]. Before any leftover is listed, the bundle passes its own
    /// delete checks: any refusal, such as a process running from it, a bundle outside the
    /// roots, or a mount point, refuses the whole operation, naming the reason, so nothing
    /// of an app that cannot be deleted is cleaned (`docs/spec/app.md`). A leftover a
    /// process runs from is refused alone. An installed app rosie cannot identify is
    /// reported, since leftovers the two apps share may be `clean`.
    ///
    /// The app itself may have no bundle-ID key: it is the app the user asked to remove, not
    /// one rosie must first identify among others, so this is not refused. Its own bundle is
    /// still `clean` and its `CFBundleName` still matches leftovers by name (aggressively,
    /// like any name-only match); it simply has no bundle ID to match leftovers by.
    pub fn scan(&self, app: &Path, settings: &Settings) -> Result<Scan, Error> {
        let ctx = self.ctx;
        let mounts = Mounts::from(settings.walk);
        let path = UserInput::path(app, settings.walk.enter_links).perform(ctx)?;
        let entry = match Entry::lstat(ctx, &path) {
            Ok(entry) => entry,
            Err(NotAnEntry::Fs(error)) => return Err(unreadable(&path, error)),
            Err(refused) => return Err(refused.into()),
        };
        let itself = match (entry.meta().kind, is_app_path(&path)) {
            (FileKind::Dir, true) => Candidate::of(entry),
            _ => None,
        };
        let Some(itself) = itself else {
            return Err(Error::NotAnApp { path });
        };
        let bundle = read_bundle(ctx, &path).map_err(|error| match error {
            Error::Fs(source) => unreadable(&path, source),
            described => described,
        })?;
        Cleanup::delete(&bundle.path)
            .verify(ctx)
            .map_err(|source| Error::Refused {
                app: bundle.path.clone(),
                source,
            })?;
        let installed = self.installed()?;
        let others: Vec<BundleId> = installed
            .apps
            .iter()
            .filter(|other| other.path != bundle.path)
            .map(|other| other.id.clone())
            .collect();
        let rule = match &bundle.id {
            Some(id) => format!("app/{}", id.as_str()),
            None => "app".to_owned(),
        };

        let mut search = Search::new(ctx, mounts, settings.aggressive);
        search.add(itself, Holds::Plain, &rule, Tier::Normal)?;

        for location in leftover_locations(ctx.paths()) {
            let tier_of = |name: &str| {
                let id_tier = bundle
                    .id
                    .as_ref()
                    .filter(|id| claims_by_id(id, name, location.holds))
                    .map(|id| id_match_tier(id, &others, name, location.holds));
                let by_name = bundle
                    .name
                    .as_ref()
                    .is_some_and(|own| claims_by_name(own, name));
                match (id_tier, by_name) {
                    (Some(tier), _) => Some(tier),
                    (None, true) => Some(Tier::Aggressive),
                    (None, false) => None,
                }
            };
            for (candidate, tier) in search.candidates(&location, tier_of)? {
                search.add(candidate, location.holds, &rule, tier)?;
            }
        }

        for (package, tier) in self.receipts(&bundle, &others)? {
            search.add_receipt(&package, tier)?;
        }
        for report in reports(ctx, &bundle)? {
            search.add_report(report);
        }
        for unidentified in &installed.unidentified {
            search.add_report(unidentified_report(unidentified, &bundle)?);
        }
        search.finish()
    }

    /// The installed apps, read on first use.
    fn installed(&self) -> Result<&Installed, Error> {
        if let Some(installed) = self.installed.get() {
            return Ok(installed);
        }
        let read = installed_apps(self.ctx)?;
        Ok(self.installed.get_or_init(|| read))
    }

    /// The installed package receipts whose IDs are bundle-ID matches of the app, each
    /// with the tier [`id_match_tier`] gives it. An app with no bundle ID matches none,
    /// and needs no receipt listing.
    fn receipts(&self, bundle: &Bundle, others: &[BundleId]) -> Result<Vec<(String, Tier)>, Error> {
        let Some(id) = &bundle.id else {
            return Ok(Vec::new());
        };
        let listed = self.packages()?;
        let matching = listed
            .lines()
            .filter(|package| claims_by_id(id, package, Holds::Plain))
            .map(|package| {
                let tier = id_match_tier(id, others, package, Holds::Plain);
                (package.to_owned(), tier)
            });
        Ok(matching.collect())
    }

    /// The installed package receipt IDs, one per line, listed on first use.
    fn packages(&self) -> Result<&str, Error> {
        if let Some(packages) = self.packages.get() {
            return Ok(packages);
        }
        let listed = self.ctx.run_tool_text(&pkgs_argv())?;
        Ok(self.packages.get_or_init(|| listed))
    }
}

/// A filesystem or `plutil` failure reading the typed bundle at `bundle`, named as such
/// so it is never mistaken for a failure unrelated to the bundle.
fn unreadable(bundle: &Path, source: fs::Error) -> Error {
    Error::UnreadableBundle {
        bundle: bundle.to_path_buf(),
        source,
    }
}

/// A bundle-ID match is `clean`, unless it also belongs to another installed app,
/// `others`: a second copy with the same ID, or an app with a longer ID that also
/// matches it (`com.foo.Bar.canary.plist` while `com.foo.Bar.canary` is installed and
/// `com.foo.Bar` is being removed). Then it is aggressive.
fn id_match_tier(id: &BundleId, others: &[BundleId], name: &str, holds: Holds) -> Tier {
    let shared = others
        .iter()
        .any(|other| claimed_by_other(id, other, name, holds));
    match shared {
        true => Tier::Aggressive,
        false => Tier::Normal,
    }
}

// receipts

pub(super) fn pkgs_argv() -> SystemArgv {
    SystemTool::PKGUTIL.argv().arg("--pkgs")
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
fn reports<B: Backend>(ctx: &FsContext<B>, bundle: &Bundle) -> Result<Vec<Report>, Error> {
    let mut found = Vec::new();
    for manual in Manual::ALL {
        let names = match list_folder(ctx, &bundle.path.join(manual.folder()))? {
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
        format!(
            "Leftovers {app} shares with {other} are not told apart, so they may be marked clean"
        ),
        "Set any item such an app still uses to keep before running the plan".to_owned(),
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
