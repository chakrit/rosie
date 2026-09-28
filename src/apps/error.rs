use std::path::PathBuf;

use thiserror::Error;

use crate::fs::{self, Argv, Exit};
use crate::{plan, processes};

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error(transparent)]
    Plan(#[from] plan::Error),

    #[error(transparent)]
    Processes(#[from] processes::Error),

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

    #[error("`{argv}` failed ({exit:?}): {stderr}")]
    Failed {
        argv: Argv,
        exit: Exit,
        stderr: String,
    },

    #[error("`{argv}` printed output that is not UTF-8")]
    NotUtf8 { argv: Argv },
}
