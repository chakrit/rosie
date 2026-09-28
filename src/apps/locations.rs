//! The folders app and orphan scans search for leftovers: the app-leftover folders
//! under `~/Library` and `/Library` in `docs/vendor/macos-filesystem-layout.md`, the
//! same ones a fresh `config.toml` seeds as roots (`docs/spec/safety.md#roots`).

use std::path::{Path, PathBuf};

use crate::plan::LaunchDomain;

/// What a leftover folder holds, which decides how its entries match and what a plan
/// does with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Holds {
    Plain,
    /// Named by bundle ID, or team-prefixed as `<team>.<id>`.
    GroupContainers,
    /// A user's launch agent plists, booted out of `gui/<uid>` before deletion.
    LaunchAgents,
    /// Launch daemon plists, booted out of `system` before deletion.
    LaunchDaemons,
}

impl Holds {
    pub(super) fn launch_domain(self, user_uid: u32) -> Option<LaunchDomain> {
        match self {
            Holds::LaunchAgents => Some(LaunchDomain::Gui(user_uid)),
            Holds::LaunchDaemons => Some(LaunchDomain::System),
            Holds::Plain | Holds::GroupContainers => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Location {
    pub path: PathBuf,
    pub holds: Holds,
}

const USER_FOLDERS: [(&str, Holds); 10] = [
    ("Library/Caches", Holds::Plain),
    ("Library/Application Support", Holds::Plain),
    ("Library/Preferences", Holds::Plain),
    ("Library/Containers", Holds::Plain),
    ("Library/Group Containers", Holds::GroupContainers),
    ("Library/Saved Application State", Holds::Plain),
    ("Library/HTTPStorages", Holds::Plain),
    ("Library/WebKit", Holds::Plain),
    ("Library/Logs", Holds::Plain),
    ("Library/LaunchAgents", Holds::LaunchAgents),
];

const SYSTEM_FOLDERS: [(&str, Holds); 2] = [
    ("/Library/LaunchDaemons", Holds::LaunchDaemons),
    ("/Library/PrivilegedHelperTools", Holds::Plain),
];

pub(super) fn leftover_locations(home: &Path) -> Vec<Location> {
    let user = USER_FOLDERS.iter().map(|(folder, holds)| Location {
        path: home.join(folder),
        holds: *holds,
    });
    let system = SYSTEM_FOLDERS.iter().map(|(folder, holds)| Location {
        path: PathBuf::from(folder),
        holds: *holds,
    });
    user.chain(system).collect()
}
