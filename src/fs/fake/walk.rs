//! The kernel path walk: how a typed path resolves to a stored entry.
//!
//! `.` and `..` apply to the folder reached so far, every symlink before the final
//! component is followed, and a non-folder before the final component is `ENOTDIR`. A
//! path ending in `/` or `/.` follows its final link too and must end at a folder. Each
//! name matches exactly or, failing that, case-insensitively, as a default APFS volume
//! does.

use std::ffi::{OsStr, OsString};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use super::{Body, State};

/// The kernel's limit on symlinks followed in one path walk (`MAXSYMLINKS`).
const MAX_SYMLINK_HOPS: usize = 32;
/// macOS `ELOOP`, so a loop fails with the same OS error as on the machine.
const ELOOP: i32 = 62;

/// Whether a path walk follows a symlink in the final component, as `stat` does, or
/// stops at the link, as `lstat` does. Links before the final component are always
/// followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FinalLink {
    Follow,
    Keep,
}

/// Where a path walk ended.
pub(super) enum Location {
    Found(PathBuf),
    /// The final component is missing; this is where it would be created. Its parent is
    /// an existing folder.
    Absent(PathBuf, Creatable),
}

/// What may be created where a path walk found nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Creatable {
    Anything,
    /// The typed path ended in `/` or `/.`, so only a folder can take the name.
    FolderOnly,
}

/// How a typed path ends. `Path::components` drops a trailing `/` and `/.`, but the
/// kernel reads both as naming a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tail {
    Name,
    Slash,
    Dot,
    DotDot,
}

impl Location {
    pub(super) fn existing(self) -> io::Result<PathBuf> {
        match self {
            Location::Found(key) => Ok(key),
            Location::Absent(..) => Err(ErrorKind::NotFound.into()),
        }
    }
}

impl Tail {
    pub(super) fn of(path: &Path) -> Tail {
        let bytes = path.as_os_str().as_encoded_bytes();
        let name_end = bytes
            .iter()
            .rposition(|&byte| byte != b'/')
            .map_or(0, |last| last + 1);
        let name_start = bytes[..name_end]
            .iter()
            .rposition(|&byte| byte == b'/')
            .map_or(0, |slash| slash + 1);
        let has_slash = name_end < bytes.len();

        match (&bytes[name_start..name_end], has_slash) {
            (b".", _) => Tail::Dot,
            (b"..", _) => Tail::DotDot,
            (_, true) => Tail::Slash,
            (_, false) => Tail::Name,
        }
    }

    pub(super) fn names_a_folder(self) -> bool {
        matches!(self, Tail::Slash | Tail::Dot)
    }

    /// `.` and `..` are never a name that can be moved or given.
    pub(super) fn is_dot_name(self) -> bool {
        matches!(self, Tail::Dot | Tail::DotDot)
    }
}

impl State {
    /// The stored path of an existing entry, not following a symlink in the final
    /// component (`lstat` semantics).
    pub(super) fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        self.locate(path, FinalLink::Keep)?.existing()
    }

    /// The stored path of an existing entry, following symlinks in the final component
    /// too (`stat` semantics).
    pub(super) fn follow(&self, path: &Path) -> io::Result<PathBuf> {
        self.locate(path, FinalLink::Follow)?.existing()
    }

    /// Walks a path as the kernel does: `.` and `..` apply to the folder reached so far,
    /// every symlink before the final component is followed, and a non-folder before the
    /// final component is `ENOTDIR`. A path ending in `/` or `/.` follows its final link
    /// too and must end at a folder. Each name matches exactly or, failing that,
    /// case-insensitively, as a default APFS volume does.
    pub(super) fn locate(&self, path: &Path, final_link: FinalLink) -> io::Result<Location> {
        if !path.has_root() {
            return Err(ErrorKind::InvalidInput.into());
        }
        let tail = Tail::of(path);
        let (final_link, creatable) = match tail.names_a_folder() {
            true => (FinalLink::Follow, Creatable::FolderOnly),
            false => (final_link, Creatable::Anything),
        };

        let mut pending: Vec<OsString> = components_reversed(path);
        let mut current = PathBuf::from("/");
        let mut hops = 0;
        while let Some(name) = pending.pop() {
            match name.to_str() {
                Some(".") => continue,
                Some("..") => {
                    current.pop();
                    continue;
                }
                _ => {}
            }

            let is_final = pending.is_empty();
            let Some(child) = self.child_named(&current, &name) else {
                return match (is_final, tail) {
                    // `x/.` looks `.` up inside `x`, so `x` must exist.
                    (true, Tail::Dot) | (false, _) => Err(ErrorKind::NotFound.into()),
                    (true, _) => Ok(Location::Absent(current.join(name), creatable)),
                };
            };

            let body = &self.node_at(&child).body;
            let follows = !is_final || final_link == FinalLink::Follow;
            match body {
                Body::Symlink(target) if follows => {
                    hops += 1;
                    if hops > MAX_SYMLINK_HOPS {
                        return Err(io::Error::from_raw_os_error(ELOOP));
                    }
                    if target.has_root() {
                        current = PathBuf::from("/");
                    }
                    pending.extend(components_reversed(target));
                }
                Body::Dir => current = child,
                _ if is_final => current = child,
                _ => return Err(ErrorKind::NotADirectory.into()),
            }
        }

        let folder_demanded = creatable == Creatable::FolderOnly;
        if folder_demanded && !self.is_dir(&current) {
            return Err(ErrorKind::NotADirectory.into());
        }
        Ok(Location::Found(current))
    }

    /// The stored path of `name` in the folder `dir`, matched exactly or, failing that,
    /// case-insensitively.
    pub(super) fn child_named(&self, dir: &Path, name: &OsStr) -> Option<PathBuf> {
        let exact = dir.join(name);
        if self.entries.contains_key(&exact) {
            return Some(exact);
        }

        let folded = name.to_string_lossy().to_lowercase();
        self.children(dir)
            .map(|(child, _)| child)
            .find(|child| folded_name(child).as_ref() == Some(&folded))
    }
}

/// The names of a path, `.` and `..` included, last first, ready to pop in order.
fn components_reversed(path: &Path) -> Vec<OsString> {
    path.components()
        .filter(|component| *component != std::path::Component::RootDir)
        .map(|component| component.as_os_str().to_owned())
        .rev()
        .collect()
}

fn folded_name(path: &Path) -> Option<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
}
