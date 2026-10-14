use std::path::PathBuf;

use thiserror::Error;

use crate::fs;
use crate::plan;
use crate::scan::{self, NotAnEntry};

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error(transparent)]
    Plan(#[from] plan::Error),

    #[error("{path} is not an app: expected a folder named <name>.app", path = path.display())]
    NotAnApp { path: PathBuf },

    #[error("{bundle} has no Info.plist: neither Contents/Info.plist nor a single \
     Wrapper/<name>.app/Info.plist", bundle = bundle.display())]
    NoInfoPlist { bundle: PathBuf },

    #[error("{plist} has no {key}", plist = plist.display())]
    MissingKey { plist: PathBuf, key: &'static str },

    #[error(
        "{plist}: {id:?} is not a bundle ID rosie can match leftovers by; \
         expected at least two dot-separated parts of letters, digits, - and _",
        plist = plist.display()
    )]
    InvalidBundleId { plist: PathBuf, id: String },

    #[error(
        "cannot read the bundle ID of the installed app {bundle}, so its leftovers cannot be \
         told from orphans: {source}",
        bundle = bundle.display()
    )]
    UnreadableApp {
        bundle: PathBuf,
        #[source]
        source: Box<Error>,
    },

    /// The scanned app's own bundle, at its real location, could not be read: a
    /// filesystem or `plutil` failure, which [`Error::Fs`] alone would not tell apart
    /// from failures unrelated to the bundle.
    #[error("cannot read the app {bundle}: {source}", bundle = bundle.display())]
    UnreadableBundle {
        bundle: PathBuf,
        #[source]
        source: fs::Error,
    },

    #[error(
        "cannot list the app folder {folder}, so the apps in it cannot be told from \
         orphans: {source}",
        folder = folder.display()
    )]
    UnlistableAppFolder {
        folder: PathBuf,
        #[source]
        source: fs::Error,
    },

    /// The app's own bundle fails a delete check, so nothing of it is cleaned
    /// (`docs/spec/app.md`).
    #[error("cannot clean {app}: {source}", app = app.display())]
    Refused {
        app: PathBuf,
        #[source]
        source: fs::Error,
    },

    #[error(
        "cannot read `launchctl list`, so the IDs of loaded launch jobs cannot be told from \
         orphans; unexpected line: {line:?}"
    )]
    LaunchJobs { line: String },

    #[error(
        "cannot read `launchctl print system`, so the IDs of loaded system jobs cannot be \
         told from orphans; unexpected line in its services block: {line:?}"
    )]
    SystemJobs { line: String },

    #[error(
        "cannot read `launchctl print system`, so the IDs of loaded system jobs cannot be \
         told from orphans; it printed no complete services block"
    )]
    SystemJobsMissing,

    #[error("{path} is a dataless placeholder; rosie does not open it", path = path.display())]
    Placeholder { path: PathBuf },

    #[error(
        "{path} is the root of a mounted volume; rosie never plans a mount point, which \
         would plan the whole volume",
        path = path.display()
    )]
    Mount { path: PathBuf },

    /// Screening or sizing a matched item met an error other than a walk skip.
    #[error(transparent)]
    Item(scan::Problem),
}

impl From<NotAnEntry> for Error {
    fn from(refused: NotAnEntry) -> Self {
        match refused {
            NotAnEntry::Fs(error) => Error::Fs(error),
            NotAnEntry::Placeholder { path } => Error::Placeholder { path },
            NotAnEntry::Mount { path } => Error::Mount { path },
        }
    }
}
