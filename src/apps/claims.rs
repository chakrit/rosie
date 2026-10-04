//! What still claims a bundle ID, so the orphan scan never lists its leftovers
//! (`docs/spec/app.md#rosie-clean-orphans`): an installed app with that ID or sharing its
//! vendor prefix, a loaded launchd job, or an installed Homebrew formula.

use std::path::Path;

use super::Error;
use super::bundle::InstalledApp;
use super::matching::mentions;
use crate::fs::{Backend, Gate, SystemArgv, SystemTool};
use crate::process;

/// The folders Homebrew installs formulae into, one folder per formula, on Apple silicon
/// and on Intel.
const CELLARS: [&str; 2] = ["/opt/homebrew/Cellar", "/usr/local/Cellar"];

/// The label prefix Homebrew gives a formula's launchd service.
const HOMEBREW_SERVICE: &str = "homebrew.mxcl.";

/// Every dot-separated ID something still claims. A leftover whose name mentions one is
/// claimed, and never an orphan.
pub(super) struct Claims {
    ids: Vec<String>,
}

impl Claims {
    /// Reads what claims IDs on this machine. A listing rosie cannot read fails: an ID
    /// it would have claimed could otherwise be listed as an orphan.
    pub fn read<B: Backend>(gate: &Gate<B>, installed: &[InstalledApp]) -> Result<Self, Error> {
        let vendors = installed.iter().map(|app| vendor_prefix(app.id.as_str()));
        let jobs = loaded_jobs(gate)?;
        let formulae = installed_formulae(gate)?;
        let services = formulae
            .into_iter()
            .map(|formula| format!("{HOMEBREW_SERVICE}{formula}"));

        let ids = vendors.chain(jobs).chain(services).collect();
        Ok(Claims { ids })
    }

    pub fn claim(&self, name: &str) -> bool {
        self.ids.iter().any(|id| mentions(id, name))
    }
}

/// `launchctl list`, which lists the jobs loaded in the invoking user's domain.
pub fn launch_jobs_argv() -> SystemArgv {
    SystemTool::LAUNCHCTL.argv().arg("list")
}

/// An app's vendor prefix: the first two dot-parts of its bundle ID. It claims the app's
/// own ID too, which always starts with it.
fn vendor_prefix(id: &str) -> String {
    id.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// The labels of the loaded launchd jobs: the third column of `launchctl list` below its
/// `PID Status Label` header.
fn loaded_jobs<B: Backend>(gate: &Gate<B>) -> Result<Vec<String>, Error> {
    let listed = process::run_tool_text(gate, &launch_jobs_argv())?;
    let mut lines = listed.lines();
    let unreadable = |line: &str| Error::LaunchJobs {
        line: line.to_owned(),
    };

    let header = lines.next().unwrap_or_default();
    if header.split('\t').collect::<Vec<_>>() != ["PID", "Status", "Label"] {
        return Err(unreadable(header));
    }
    lines
        .map(|line| match line.split('\t').collect::<Vec<_>>()[..] {
            [_, _, label] if !label.is_empty() => Ok(label.to_owned()),
            _ => Err(unreadable(line)),
        })
        .collect()
}

/// The names of the installed Homebrew formulae: the folders in each Cellar there is.
fn installed_formulae<B: Backend>(gate: &Gate<B>) -> Result<Vec<String>, Error> {
    let mut formulae = Vec::new();
    for cellar in CELLARS {
        let names = match gate.read_dir(Path::new(cellar)) {
            Ok(names) => names,
            Err(absent) if absent.has_vanished() => continue,
            Err(error) => return Err(error.into()),
        };
        // A formula named in bytes that are not UTF-8 could only claim a leftover named
        // the same way, and the scan never considers such a name.
        let readable = names.into_iter().filter_map(|name| name.into_string().ok());
        formulae.extend(readable);
    }
    Ok(formulae)
}
