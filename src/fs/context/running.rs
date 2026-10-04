//! The programs running when a context first checks a delete: every file a process has
//! mapped as program text, from one `lsof` snapshot per context
//! (`docs/spec/safety.md#running-processes`,
//! `docs/decisions/2026-10-04-running-check-in-fs-layer.md`).
//!
//! `lsof` reports the kernel's path for each mapped file, so a program launched by a bare
//! name, through a symlink, or under a rewritten process title is still found.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use crate::fs::backend::{SystemArgv, SystemTool};

/// `lsof -nP -d txt -F pn`: program text and mapped files only, as `p<pid>` and
/// `n<path>` fields, with no name or port lookups.
pub fn running_argv() -> SystemArgv {
    SystemTool::LSOF
        .argv()
        .arg("-nP")
        .arg("-d")
        .arg("txt")
        .arg("-F")
        .arg("pn")
}

/// A process with a file mapped as program text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Program {
    pub(super) pid: u32,
    pub(super) path: PathBuf,
}

/// Every mapped program file of the snapshot, by path, with one process that maps it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Running {
    programs: BTreeMap<PathBuf, u32>,
}

impl Running {
    /// Parses `lsof -F pn` output: a `p` line starts each process, an `n` line names one
    /// of its files, and the `f` line lsof adds before each file is passed over. Any
    /// other line fails the parse, so output rosie does not understand never reads as
    /// "nothing runs".
    pub fn parse(text: &[u8]) -> Result<Running, String> {
        let mut programs = BTreeMap::new();
        let mut pid = None;
        for line in text.split(|&byte| byte == b'\n') {
            match line.split_first() {
                None => {}
                Some((b'p', digits)) => pid = Some(parse_pid(digits)?),
                Some((b'f', _)) => {}
                Some((b'n', path)) => {
                    let Some(pid) = pid else {
                        return Err(unreadable(line));
                    };
                    let path = PathBuf::from(OsStr::from_bytes(path));
                    programs.entry(path).or_insert(pid);
                }
                Some(_) => return Err(unreadable(line)),
            }
        }
        Ok(Running { programs })
    }

    /// A program whose file is `path` or lies inside it, compared component by
    /// component. Paths inside `path` sort right after it, so only the first one at or
    /// after it can match.
    pub fn inside(&self, path: &Path) -> Option<Program> {
        let (program, pid) = self.programs.range(path.to_path_buf()..).next()?;
        program.starts_with(path).then(|| Program {
            pid: *pid,
            path: program.clone(),
        })
    }
}

fn parse_pid(digits: &[u8]) -> Result<u32, String> {
    std::str::from_utf8(digits)
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| unreadable(digits))
}

fn unreadable(line: &[u8]) -> String {
    format!(
        "cannot read `{}` output line {:?}",
        running_argv(),
        String::from_utf8_lossy(line)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LSOF: &str = "p650\nftxt\nn/System/Library/CoreServices/loginwindow.app/Contents/MacOS/loginwindow\n\
                        p33012\nftxt\nn/opt/homebrew/Cellar/tmux/3.7c/bin/tmux\nftxt\nn/usr/lib/dyld\n";

    #[test]
    fn finds_a_program_inside_a_folder_whatever_name_launched_it() {
        let running = Running::parse(LSOF.as_bytes()).expect("lsof output parses");

        let found = running.inside(Path::new("/opt/homebrew/Cellar/tmux"));

        assert_eq!(
            found,
            Some(Program {
                pid: 33012,
                path: PathBuf::from("/opt/homebrew/Cellar/tmux/3.7c/bin/tmux"),
            })
        );
    }

    #[test]
    fn matches_whole_components_only() {
        let running = Running::parse(LSOF.as_bytes()).expect("lsof output parses");

        assert_eq!(running.inside(Path::new("/opt/homebrew/Cellar/tmu")), None);
        assert_eq!(
            running.inside(Path::new("/opt/homebrew/Cellar/tmux2")),
            None
        );
        assert!(running.inside(Path::new("/usr/lib/dyld")).is_some());
    }

    #[test]
    fn refuses_output_it_does_not_understand() {
        for text in ["p12\nx/what\n", "n/before/any/process\n", "pnot-a-pid\n"] {
            let parsed = Running::parse(text.as_bytes());

            assert!(parsed.is_err(), "{text:?}: {parsed:?}");
        }
    }
}
