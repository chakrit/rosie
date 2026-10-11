//! The version check of the system tools rosie runs
//! (`docs/spec/safety.md#system-tool-versions`): before a process first runs a tool, it
//! reads the tool's version, and a version rosie was not tested with draws a warning. The
//! run goes on either way.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use super::backend::{
    Argv, Backend, CommandOutput, Exit, Metadata, SystemTool, VersionFormat, VersionSource,
};

/// The macOS version rosie was tested on; it stands for the tools that report no version
/// of their own.
pub const TESTED_MACOS: &str = "26.6.2";

/// A backend that checks each system tool's version once, before the first command that
/// runs it, and keeps the warnings until they are taken. Wrapping the one backend a
/// process uses makes the check once per process.
pub struct VersionCheck<B: Backend> {
    inner: B,
    state: Mutex<Checked>,
}

#[derive(Default)]
struct Checked {
    tools: Vec<&'static str>,
    macos: Option<Read>,
    warnings: Vec<UntestedVersion>,
}

/// A version rosie was not tested with, or one it could not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UntestedVersion {
    Tool {
        tool: &'static str,
        found: String,
        tested: &'static str,
    },
    MacOs {
        tool: &'static str,
        found: String,
    },
    Unreadable {
        tool: &'static str,
        reason: String,
    },
}

/// The outcome of reading one version.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Read {
    Found(String),
    Unreadable(String),
}

impl<B: Backend> VersionCheck<B> {
    pub fn new(inner: B) -> Self {
        VersionCheck {
            inner,
            state: Mutex::new(Checked::default()),
        }
    }

    /// The warnings gathered since the last call.
    pub fn take_warnings(&self) -> Vec<UntestedVersion> {
        std::mem::take(&mut self.lock().warnings)
    }

    /// Checks the tool `program` names, the first time a command runs it. A program that
    /// is no system tool, such as a tool rule's own shell, is not checked.
    fn check(&self, program: &OsStr) {
        let Some(tool) = SystemTool::at_path(program) else {
            return;
        };
        let mut state = self.lock();
        if state.tools.contains(&tool.path()) {
            return;
        }
        state.tools.push(tool.path());

        let warning = match tool.version() {
            VersionSource::Own {
                args,
                format,
                tested,
            } => self.own_warning(tool, args, format, tested),
            VersionSource::MacOs => self.macos_warning(&mut state, tool),
            VersionSource::IsMacOs => None,
        };

        state.warnings.extend(warning);
    }

    fn own_warning(
        &self,
        tool: SystemTool,
        args: &[&str],
        format: VersionFormat,
        tested: &'static str,
    ) -> Option<UntestedVersion> {
        let argv = args.iter().fold(tool.argv(), |argv, arg| argv.arg(*arg));
        match self.read_version(&argv.into(), |text| format.read(text)) {
            Read::Found(found) if found == tested => None,
            Read::Found(found) => Some(UntestedVersion::Tool {
                tool: tool.path(),
                found,
                tested,
            }),
            Read::Unreadable(reason) => Some(UntestedVersion::Unreadable {
                tool: tool.path(),
                reason,
            }),
        }
    }

    /// The macOS version is read once and stands for every tool with no version of its
    /// own; each such tool draws its own warning.
    fn macos_warning(&self, state: &mut Checked, tool: SystemTool) -> Option<UntestedVersion> {
        let argv = SystemTool::SW_VERS.argv().arg("-productVersion");
        let macos = state
            .macos
            .get_or_insert_with(|| self.read_version(&argv.into(), read_macos));

        match macos {
            Read::Found(found) if found == TESTED_MACOS => None,
            Read::Found(found) => Some(UntestedVersion::MacOs {
                tool: tool.path(),
                found: found.clone(),
            }),
            Read::Unreadable(reason) => Some(UntestedVersion::Unreadable {
                tool: tool.path(),
                reason: format!("the macOS version: {reason}"),
            }),
        }
    }

    /// Runs a version probe and reads its stdout and stderr together, as the tools
    /// print their version to either.
    fn read_version(&self, argv: &Argv, read: impl Fn(&str) -> Option<&str>) -> Read {
        let output = match self.inner.run(argv) {
            Ok(output) => output,
            Err(error) => return Read::Unreadable(format!("{argv}: {error}")),
        };
        if !output.exit.success() {
            return Read::Unreadable(format!("{argv} failed"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let text = format!("{stdout}{stderr}");
        match read(&text) {
            Some(found) => Read::Found(found.to_owned()),
            None => Read::Unreadable(format!("{argv} printed no version rosie can read")),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Checked> {
        self.state
            .lock()
            .expect("no thread panics while checking a version")
    }
}

/// `sw_vers -productVersion` prints the version alone, such as `26.6.2`.
fn read_macos(text: &str) -> Option<&str> {
    let version = text.trim();
    let well_formed = !version.is_empty()
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    well_formed.then_some(version)
}

impl fmt::Display for UntestedVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UntestedVersion::Tool {
                tool,
                found,
                tested,
            } => write!(
                f,
                "warning: {tool} is version {found}; rosie was tested with {tested}"
            ),
            UntestedVersion::MacOs { tool, found } => write!(
                f,
                "warning: {tool} comes with macOS {found}; rosie was tested on macOS {TESTED_MACOS}"
            ),
            UntestedVersion::Unreadable { tool, reason } => {
                write!(f, "warning: cannot read the version of {tool}: {reason}")
            }
        }
    }
}

impl VersionFormat {
    /// The version in a tool's version output, if the output reads as this format.
    pub fn read(self, text: &str) -> Option<&str> {
        let found = match self {
            VersionFormat::Bash => after_in_first_line(text, "GNU bash, version ", '('),
            VersionFormat::Launchctl => {
                after_in_first_line(text, "Darwin Bootstrapper Version ", ':')
            }
            VersionFormat::Lsof => text
                .lines()
                .find_map(|line| line.trim_start().strip_prefix("revision:"))
                .map(str::trim),
            VersionFormat::Sudo => after_in_first_line(text, "Sudo version ", '\n'),
        };
        found.filter(|version| !version.is_empty() && !version.contains(char::is_whitespace))
    }
}

/// What follows `prefix` at the start of the first line, up to `end` or the line's end.
fn after_in_first_line<'t>(text: &'t str, prefix: &str, end: char) -> Option<&'t str> {
    let first = text.lines().next()?;
    let rest = first.strip_prefix(prefix)?;
    let version = rest.split_once(end).map_or(rest, |(head, _)| head);
    Some(version.trim())
}

impl<B: Backend> Backend for VersionCheck<B> {
    fn read_dir(&self, path: &Path) -> io::Result<Vec<OsString>> {
        self.inner.read_dir(path)
    }

    fn lstat(&self, path: &Path) -> io::Result<Metadata> {
        self.inner.lstat(path)
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }

    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read_file(path)
    }

    fn write_file(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.inner.write_file(path, contents)
    }

    fn create_file(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.inner.create_file(path, contents)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_dir_all(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_file(path)
    }

    fn remove_empty_dir(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_empty_dir(path)
    }

    fn set_mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        self.inner.set_mode(path, mode)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner.rename(from, to)
    }

    fn run(&self, argv: &Argv) -> io::Result<CommandOutput> {
        self.check(argv.program());
        self.inner.run(argv)
    }

    fn run_streaming<F: FnMut(&str)>(&self, argv: &Argv, on_line: F) -> io::Result<Exit> {
        self.check(argv.program());
        self.inner.run_streaming(argv, on_line)
    }

    fn sudo(&self, argv: &Argv, stdin: &[u8]) -> io::Result<CommandOutput> {
        self.check(OsStr::new(SystemTool::SUDO.path()));
        self.inner.sudo(argv, stdin)
    }
}

#[cfg(test)]
mod tests;
