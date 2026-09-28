//! App bundles: their identity, read from `Info.plist` with `plutil`
//! (`docs/spec/app.md`), and the bundles installed in a set of folders.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use super::listing::{Listing, list_folder};
use super::{Error, run_for_text};
use crate::fs::{self, Argv, Backend, FileKind, Gate};
use crate::plan::Report;

const ID_KEY: &str = "CFBundleIdentifier";
const NAME_KEY: &str = "CFBundleName";

/// What `plutil -extract` prints on stderr for a key the plist does not hold.
const NO_VALUE: &str = "No value at that key path";

/// A reverse-DNS bundle identifier: at least two dot-separated parts, each non-empty
/// and made of ASCII letters, digits, `-`, and `_`. A single part such as `com` would
/// claim every `com.*` leftover, so it is never an identifier rosie matches by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BundleId(String);

impl BundleId {
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.split('.').collect();
        let valid = parts.len() >= 2 && parts.iter().all(|part| is_id_part(part));
        valid.then(|| BundleId(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A non-empty `CFBundleName`. `plutil` can print an empty value for a bundle that sets
/// the key to `""`; parsing rejects that case so no `Bundle` ever holds an empty name,
/// and no name-only match can be built from one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BundleName(String);

impl BundleName {
    pub fn parse(text: String) -> Option<Self> {
        (!text.is_empty()).then_some(BundleName(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub(super) fn is_id_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// An app bundle whose leftovers `rosie clean app` searches for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Bundle {
    pub path: PathBuf,
    /// The bundle's own `CFBundleIdentifier`, when its `Info.plist` holds one. A scanned
    /// app with no bundle-ID key still owns a plan: `read_bundle` reports it here as
    /// `None` rather than failing, since it is the app the user typed by path, not one
    /// rosie must first identify among others.
    pub id: Option<BundleId>,
    /// `CFBundleName`, when the plist holds a non-empty one; name-only matches use it.
    pub name: Option<BundleName>,
}

impl Bundle {
    /// How reports name the app: its `CFBundleName`, else the bundle's file name with
    /// its `.app` extension stripped.
    pub fn display_name(&self) -> String {
        self.name
            .as_ref()
            .map(|name| name.as_str().to_owned())
            .unwrap_or_else(|| bundle_stem(&self.path))
    }
}

/// `path`'s file name with a trailing `.app` removed, if it has one.
fn bundle_stem(path: &Path) -> String {
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    match file_name.strip_suffix(".app") {
        Some(stem) => stem.to_owned(),
        None => file_name.into_owned(),
    }
}

/// Reads the identity of the bundle at `path`, a folder already checked.
pub(super) fn read_bundle<B: Backend>(gate: &Gate<B>, path: &Path) -> Result<Bundle, Error> {
    let plist = locate_info_plist(gate, path)?;
    let id = match read_id_at(gate, &plist) {
        Ok(id) => Some(id),
        Err(Error::MissingKey { .. }) => None,
        Err(error) => return Err(error),
    };
    let name = match extract(gate, &plist, NAME_KEY) {
        Ok(name) => BundleName::parse(name),
        Err(Error::MissingKey { .. }) => None,
        Err(error) => return Err(error),
    };

    Ok(Bundle {
        path: path.to_path_buf(),
        id,
        name,
    })
}

fn read_id<B: Backend>(gate: &Gate<B>, bundle: &Path) -> Result<BundleId, Error> {
    let plist = locate_info_plist(gate, bundle)?;
    read_id_at(gate, &plist)
}

fn read_id_at<B: Backend>(gate: &Gate<B>, plist: &Path) -> Result<BundleId, Error> {
    let text = extract(gate, plist, ID_KEY)?;
    let plist = plist.to_path_buf();
    BundleId::parse(&text).ok_or(Error::InvalidBundleId { plist, id: text })
}

/// Where a Mac app keeps its `Info.plist`.
pub(super) fn info_plist(bundle: &Path) -> PathBuf {
    bundle.join("Contents/Info.plist")
}

/// The `Info.plist` of a bundle, `lstat`ed so a symlink in its path is refused rather
/// than followed by `plutil`: `Contents/Info.plist` for a Mac app, or, for an iPhone or
/// iPad app installed on a Mac with Apple silicon, the plist of the one `.app` in its
/// `Wrapper/` folder. Such a bundle's `WrappedBundle` symlink is never used.
fn locate_info_plist<B: Backend>(gate: &Gate<B>, bundle: &Path) -> Result<PathBuf, Error> {
    let no_info_plist = || Error::NoInfoPlist {
        bundle: bundle.to_path_buf(),
    };

    let mac = info_plist(bundle);
    match gate.lstat_without_symlinks(&mac) {
        Ok(_) => return Ok(mac),
        Err(error) if error.has_vanished() => {}
        Err(error) => return Err(error.into()),
    }

    let wrapper = bundle.join("Wrapper");
    let names = match list_folder(gate, &wrapper)? {
        Listing::Entries(names) => names,
        // A missing `Wrapper/` folder holds no apps, which the length check below
        // already refuses with the same error.
        Listing::NoFolder => Vec::new(),
        Listing::Denied(error) => return Err(error.into()),
    };
    let apps: Vec<PathBuf> = names
        .into_iter()
        .map(|name| wrapper.join(name))
        .filter(|path| is_app_path(path))
        .collect();
    let [wrapped] = apps.as_slice() else {
        return Err(no_info_plist());
    };

    let plist = wrapped.join("Info.plist");
    match gate.lstat_without_symlinks(&plist) {
        Ok(_) => Ok(plist),
        Err(error) if error.has_vanished() => Err(no_info_plist()),
        Err(error) => Err(error.into()),
    }
}

/// `plutil -extract <key> raw -o - <plist>`, which prints the value and a newline.
pub(super) fn extract_argv(plist: &Path, key: &str) -> Argv {
    Argv::new("plutil")
        .arg("-extract")
        .arg(key)
        .arg("raw")
        .arg("-o")
        .arg("-")
        .arg(plist)
}

/// One string value of an `Info.plist` found by [`locate_info_plist`]. A missing key is
/// [`Error::MissingKey`].
fn extract<B: Backend>(gate: &Gate<B>, plist: &Path, key: &'static str) -> Result<String, Error> {
    let text = match run_for_text(gate, extract_argv(plist, key)) {
        Err(Error::Failed { stderr, .. }) if stderr.contains(NO_VALUE) => {
            let plist = plist.to_path_buf();
            return Err(Error::MissingKey { plist, key });
        }
        result => result?,
    };
    Ok(text.strip_suffix('\n').unwrap_or(&text).to_owned())
}

// installed apps

/// An app found under `/Applications`, `~/Applications`, or `/System/Applications`
/// (`docs/spec/app.md#rosie-clean-orphans`).
pub(super) struct InstalledApp {
    pub path: PathBuf,
    pub id: BundleId,
}

/// An installed app rosie cannot identify: a bundle whose ID it cannot read, or an app
/// folder it may not list. Each scan mode decides what that means, never passing it over
/// silently.
pub(super) enum Unidentified {
    Bundle { path: PathBuf, cause: Error },
    Folder(Unlistable),
}

/// An app folder rosie may not list.
pub(super) struct Unlistable {
    pub path: PathBuf,
    pub cause: fs::Error,
}

impl Unidentified {
    pub(super) fn path(&self) -> &Path {
        match self {
            Unidentified::Bundle { path, .. } | Unidentified::Folder(Unlistable { path, .. }) => {
                path
            }
        }
    }

    pub(super) fn into_error(self) -> Error {
        match self {
            Unidentified::Bundle { path, cause } => Error::UnreadableApp {
                bundle: path,
                source: Box::new(cause),
            },
            Unidentified::Folder(Unlistable { path, cause }) => Error::UnlistableAppFolder {
                folder: path,
                source: cause,
            },
        }
    }
}

/// What the app folders hold.
pub(super) struct Installed {
    pub apps: Vec<InstalledApp>,
    /// A bundle whose `Info.plist` has no bundle-ID key: a definite answer, not a failed
    /// read. It has no bundle ID to claim or demote a leftover by, so it never blocks a
    /// scan; any leftover it has may appear among the orphans instead. The orphan scan
    /// reports it as a note for that reason; the app scan does not, since it has nothing
    /// to say about a leftover the app being scanned already claims by its own ID.
    pub no_id: Vec<PathBuf>,
    pub unidentified: Vec<Unidentified>,
}

/// The installed apps and their bundle IDs, read in parallel.
pub(super) fn installed_apps<B>(gate: &Gate<B>, home: &Path) -> Result<Installed, Error>
where
    B: Backend + Sync,
{
    let folders = [
        PathBuf::from("/Applications"),
        home.join("Applications"),
        PathBuf::from("/System/Applications"),
    ];
    let found = bundles_in(gate, &folders)?;

    let reads: Vec<(PathBuf, Result<BundleId, Error>)> = found
        .bundles
        .into_par_iter()
        .map(|path| {
            let id = read_id(gate, &path);
            (path, id)
        })
        .collect();

    let mut installed = Installed {
        apps: Vec::new(),
        no_id: Vec::new(),
        unidentified: found
            .unlistable
            .into_iter()
            .map(Unidentified::Folder)
            .collect(),
    };
    for (path, read) in reads {
        match read {
            Ok(id) => installed.apps.push(InstalledApp { path, id }),
            Err(Error::MissingKey { .. }) => installed.no_id.push(path),
            Err(cause) => installed
                .unidentified
                .push(Unidentified::Bundle { path, cause }),
        }
    }
    Ok(installed)
}

/// The `.app` folders found in a set of app folders, and the folders rosie may not list.
pub(super) struct Found {
    pub bundles: Vec<PathBuf>,
    pub unlistable: Vec<Unlistable>,
}

/// A folder found while looking for installed apps.
enum Subfolder {
    App(PathBuf),
    Plain(PathBuf),
}

/// The `.app` folders directly in `folders`, and one level down in plain subfolders such
/// as `/Applications/Utilities`, sorted. Symlinked bundles are not followed. A missing
/// folder holds none; a folder rosie may not list is recorded as unlistable, since the
/// answer is incomplete without it.
pub(super) fn bundles_in<B: Backend>(gate: &Gate<B>, folders: &[PathBuf]) -> Result<Found, Error> {
    let mut found = Found {
        bundles: Vec::new(),
        unlistable: Vec::new(),
    };
    for folder in folders {
        for subfolder in found.list_subfolders_recording_unlistable(gate, folder)? {
            let apps = match subfolder {
                Subfolder::App(path) => vec![path],
                Subfolder::Plain(path) => found.apps_directly_in(gate, &path)?,
            };
            found.bundles.extend(apps);
        }
    }

    found.bundles.sort();
    Ok(found)
}

impl Found {
    /// The `.app` folders directly in `folder`; plain folders in it are not searched.
    fn apps_directly_in<B: Backend>(
        &mut self,
        gate: &Gate<B>,
        folder: &Path,
    ) -> Result<Vec<PathBuf>, Error> {
        let subfolders = self.list_subfolders_recording_unlistable(gate, folder)?;
        let apps = subfolders
            .into_iter()
            .filter_map(|subfolder| match subfolder {
                Subfolder::App(path) => Some(path),
                Subfolder::Plain(_) => None,
            });
        Ok(apps.collect())
    }

    /// The folders in `folder`. One that vanishes between listing and `lstat` is gone. A
    /// `folder` rosie may not list is recorded on `self.unlistable`, not returned, since
    /// the caller has no folder to report back.
    fn list_subfolders_recording_unlistable<B: Backend>(
        &mut self,
        gate: &Gate<B>,
        folder: &Path,
    ) -> Result<Vec<Subfolder>, Error> {
        let names = match list_folder(gate, folder)? {
            Listing::Entries(names) => names,
            Listing::NoFolder => return Ok(Vec::new()),
            Listing::Denied(cause) => {
                let path = folder.to_path_buf();
                self.unlistable.push(Unlistable { path, cause });
                return Ok(Vec::new());
            }
        };

        let mut found = Vec::new();
        for name in names {
            let path = folder.join(name);
            let kind = match gate.lstat(&path) {
                Ok(meta) => meta.kind,
                Err(error) if error.has_vanished() => continue,
                Err(error) => return Err(error.into()),
            };
            if kind != FileKind::Dir {
                continue;
            }
            found.push(match is_app_path(&path) {
                true => Subfolder::App(path),
                false => Subfolder::Plain(path),
            });
        }
        Ok(found)
    }
}

pub(super) fn is_app_path(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("app"))
}

/// An installed app whose `Info.plist` has no bundle-ID key ([`Installed::no_id`]). No
/// leftover is matched to it by ID, so it is only a note, not a reason to fail a scan.
pub(super) fn no_id_report(path: &Path) -> Result<Report, Error> {
    let subject = format!(
        "the installed app {} has no bundle ID, so no leftover is matched to it by ID; \
         any it has may appear among the orphans",
        path.display()
    );
    let steps = vec!["Nothing to do: no leftovers are matched to this app".to_owned()];
    Ok(Report::new(subject, steps)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_id_has_two_or_more_well_formed_parts() {
        let parsed = |text| BundleId::parse(text).is_some();

        assert!(parsed("com.foo.Bar"));
        assert!(parsed("org.my-tool.App_2"));
        assert!(parsed("io.x"));
        assert!(!parsed("com"));
        assert!(!parsed(""));
        assert!(!parsed("com..Bar"));
        assert!(!parsed("com.foo."));
        assert!(!parsed(".com.foo"));
        assert!(!parsed("com.foo/Bar"));
        assert!(!parsed("com.foo Bar"));
    }

    /// The fake answers `plutil` by this same argv, so only this test pins what the real
    /// tool is asked: the raw value of one key, printed to standard output.
    #[test]
    fn plutil_is_asked_for_one_raw_value_on_standard_output() {
        let plist = Path::new("/Applications/Bar.app/Contents/Info.plist");

        let argv = extract_argv(plist, ID_KEY);

        assert_eq!(
            argv.to_string(),
            "plutil -extract CFBundleIdentifier raw -o - /Applications/Bar.app/Contents/Info.plist"
        );
    }
}
