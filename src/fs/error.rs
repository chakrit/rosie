use std::io;
use std::path::PathBuf;

use thiserror::Error;

use super::backend::Argv;
use super::gate::Exposure;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{op} {path}: {source}", op = op.label(), path = path.display())]
    Io {
        op: Op,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("cannot run `{argv}`: {source}")]
    Command {
        argv: Argv,
        #[source]
        source: io::Error,
    },

    #[error("{name:?} is not the name of one entry in a folder")]
    NotAnEntryName { name: std::ffi::OsString },

    #[error("{path} is not an absolute path", path = path.display())]
    Relative { path: PathBuf },

    #[error("{path} is outside the allowed roots", path = path.display())]
    OutsideRoots { path: PathBuf },

    #[error(
        "{path} passes through the symlink {link}; {real}",
        path = path.display(),
        link = link.display(),
    )]
    Symlink {
        path: PathBuf,
        link: PathBuf,
        real: RealPath,
    },

    #[error(
        "{typed} does not match the spelling on disk; use {real} instead",
        typed = typed.display(),
        real = real.display(),
    )]
    Misspelled { typed: PathBuf, real: PathBuf },

    #[error("{path} is a mount of another volume; rosie does not delete across volumes", path = path.display())]
    CrossesVolume { path: PathBuf },

    #[error(
        "{path} is a dataless cloud placeholder; rosie neither downloads nor deletes it",
        path = path.display(),
    )]
    Placeholder { path: PathBuf },

    #[error("{folder} {exposure}", folder = folder.display())]
    NotRootControlled { folder: PathBuf, exposure: Exposure },

    #[error("cannot check the folders above it: {0}")]
    AboveUnchecked(#[source] Box<Error>),

    #[error("{path} is outside rosie's own folders", path = path.display())]
    OutsideOwnData { path: PathBuf },

    #[error(
        "{path} is one of rosie's own folders; only entries inside it can be deleted or moved",
        path = path.display(),
    )]
    OwnFolderItself { path: PathBuf },
}

/// The path a symlink refusal names instead: every link along it replaced by its
/// target, or the reason that could not be done.
#[derive(Debug, Error)]
pub enum RealPath {
    #[error("use the real path {path} instead", path = .0.display())]
    Resolved(PathBuf),

    #[error("its real path cannot be determined: {0}")]
    Unresolved(#[source] Box<Error>),
}

/// A filesystem primitive, named in errors and in the fake's injected failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    ReadDir,
    Lstat,
    ReadLink,
    ReadFile,
    WriteFile,
    CreateFile,
    CreateDir,
    RemoveFile,
    RemoveDir,
    SetMode,
    Rename,
}

impl Op {
    pub fn label(self) -> &'static str {
        match self {
            Op::ReadDir => "cannot list",
            Op::Lstat => "cannot inspect",
            Op::ReadLink => "cannot read link",
            Op::ReadFile => "cannot read",
            Op::WriteFile => "cannot write",
            Op::CreateFile => "cannot create",
            Op::CreateDir => "cannot create folder",
            Op::RemoveFile => "cannot remove",
            Op::RemoveDir => "cannot remove folder",
            Op::SetMode => "cannot change permissions of",
            Op::Rename => "cannot rename",
        }
    }
}
