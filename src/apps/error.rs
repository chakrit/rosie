use std::path::PathBuf;

use thiserror::Error;

use crate::fs;
use crate::scan::{self, NotAnEntry};
use crate::{plan, process};

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error(transparent)]
    Plan(#[from] plan::Error),

    #[error(transparent)]
    Process(#[from] process::Error),

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

    #[error(
        "{app} is running (process {pid}); quit it, then scan again",
        app = app.display()
    )]
    Running { app: PathBuf, pid: u32 },

    #[error("{path} is a dataless placeholder; rosie does not open it", path = path.display())]
    Placeholder { path: PathBuf },

    #[error(
        "{path} is the root of a mounted volume; rosie does not scan or delete across volumes",
        path = path.display()
    )]
    Mount { path: PathBuf },

    /// Sizing a planned item met an error other than a walk skip.
    #[error(transparent)]
    Sizing(scan::Problem),
}

impl From<NotAnEntry> for Error {
    fn from(refused: NotAnEntry) -> Self {
        match refused {
            NotAnEntry::Fs(error) => Error::Fs(error),
            NotAnEntry::Placeholder { path } => Error::Placeholder { path },
        }
    }
}
