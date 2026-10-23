//! What still claims a bundle ID, so the orphan scan never lists its leftovers
//! (`docs/spec/app.md#rosie-clean-orphans`): an installed app with that ID or sharing its
//! vendor prefix, a loaded launchd job, or an installed Homebrew formula.

use std::path::Path;

use super::Error;
use super::bundle::InstalledApp;
use super::matching::mentions;
use crate::fs::{Backend, Cleanup, FsContext, FsOp, SystemArgv, SystemTool};

/// The folders Homebrew installs formulae into, one folder per formula, on Apple silicon
/// and on Intel.
const CELLARS: [&str; 2] = ["/opt/homebrew/Cellar", "/usr/local/Cellar"];

/// The lines of `launchctl print system` that open and close its loaded-jobs block, one
/// tab in from the domain's own braces.
const SERVICES_OPEN: &str = "\tservices = {";
const BLOCK_CLOSE: &str = "\t}";

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
    pub fn read<B: Backend>(ctx: &FsContext<B>, installed: &[InstalledApp]) -> Result<Self, Error> {
        let vendors = installed.iter().map(|app| vendor_prefix(app.id.as_str()));
        let user_jobs = loaded_jobs(ctx)?;
        let system_jobs = system_jobs(ctx)?;
        let formulae = installed_formulae(ctx)?;
        let services = formulae
            .into_iter()
            .map(|formula| format!("{HOMEBREW_SERVICE}{formula}"));

        let ids = vendors
            .chain(user_jobs)
            .chain(system_jobs)
            .chain(services)
            .collect();
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

/// `launchctl print system`, which lists the jobs loaded in the system domain without
/// root. Its output is not a stable format, so anything unexpected in the part rosie
/// reads fails the scan.
pub fn system_jobs_argv() -> SystemArgv {
    SystemTool::LAUNCHCTL.argv().arg("print").arg("system")
}

/// An app's vendor prefix: the first two dot-parts of its bundle ID. It claims the app's
/// own ID too, which always starts with it.
fn vendor_prefix(id: &str) -> String {
    id.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// The labels of the loaded launchd jobs: the third column of `launchctl list` below its
/// `PID Status Label` header.
fn loaded_jobs<B: Backend>(ctx: &FsContext<B>) -> Result<Vec<String>, Error> {
    let listed = ctx.run_tool_text(&launch_jobs_argv())?;
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

/// The labels of the jobs loaded in the system domain: the last column of each
/// `<pid> <status> <label>` line in the `services` block of `launchctl print system`.
/// Every other block, `disabled services` included, names jobs that need not be loaded.
fn system_jobs<B: Backend>(ctx: &FsContext<B>) -> Result<Vec<String>, Error> {
    let printed = ctx.run_tool_text(&system_jobs_argv())?;
    let unreadable = |line: &str| Error::SystemJobs {
        line: line.to_owned(),
    };

    let lines: Vec<&str> = printed.lines().collect();
    let Some(open) = lines.iter().position(|line| *line == SERVICES_OPEN) else {
        return Err(Error::SystemJobsMissing);
    };
    let after_open = &lines[open + 1..];
    let Some(close) = after_open.iter().position(|line| *line == BLOCK_CLOSE) else {
        return Err(Error::SystemJobsMissing);
    };

    after_open[..close]
        .iter()
        .map(
            |line| match line.split_whitespace().collect::<Vec<_>>()[..] {
                [pid, _, label] if pid.bytes().all(|b| b.is_ascii_digit()) => Ok(label.to_owned()),
                _ => Err(unreadable(line)),
            },
        )
        .collect()
}

/// The names of the installed Homebrew formulae: the folders in each Cellar there is.
fn installed_formulae<B: Backend>(ctx: &FsContext<B>) -> Result<Vec<String>, Error> {
    let mut formulae = Vec::new();
    for cellar in CELLARS {
        let names = match Cleanup::read_dir(Path::new(cellar)).perform(ctx) {
            Ok(names) => names,
            Err(absent) if absent.has_vanished() => continue,
            Err(error) => return Err(error.into()),
        };
        // A formula named in bytes that are not UTF-8 could only claim a leftover named
        // the same way, and the scan never considers such a name.
        let readable = names
            .into_iter()
            .filter_map(|entry| entry.name.into_string().ok());
        formulae.extend(readable);
    }
    Ok(formulae)
}
