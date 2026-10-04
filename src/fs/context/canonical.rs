//! The symlink ban and the spelling check (`docs/spec/safety.md#symlinks`): a path is
//! accepted only when it is its own canonical form on disk. `canonicalize` is the
//! detector; its result is never used in place of the path given.

use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::{FsContext, resolve_dots};
use crate::fs::backend::{Backend, FileKind, Metadata};
use crate::fs::error::{Error, Op, RealPath};
use crate::fs::links::EnterLinks;

/// What the disk makes of a path that rosie normalized by text.
enum Canonical {
    /// The canonical form: the deepest existing ancestor canonicalized, with the missing
    /// rest of the path appended as given.
    Resolved(PathBuf),
    /// `link` exists but cannot be canonicalized: a symlink whose target is missing.
    Dangling { link: PathBuf, source: io::Error },
}

impl<B: Backend> FsContext<B> {
    /// Resolves `.` and `..` by text and returns the path when it is its own canonical
    /// form. A path through a symlink is refused naming the link and the real path; one
    /// whose case or Unicode form differs from the disk is refused naming the spelling
    /// on disk. A path, or the tail of one, that does not exist is accepted: nothing
    /// missing can be a link.
    pub(in crate::fs) fn check_real(&self, path: &Path) -> Result<PathBuf, Error> {
        let path = resolve_dots(path)?;

        match self.canonical(&path)? {
            Canonical::Resolved(real) if real == path => Ok(path),
            Canonical::Resolved(real) => Err(self.mismatch(path, real)),
            Canonical::Dangling { link, source } => {
                let source = Error::Io {
                    op: Op::Canonicalize,
                    path: link.clone(),
                    source,
                };
                Err(Error::Symlink {
                    path,
                    link,
                    real: RealPath::Unresolved(Box::new(source)),
                })
            }
        }
    }

    /// Where a path rosie is given is on disk. With `links` allowing a symlink in it, a
    /// path whose canonical form differs only by passing through one is taken to its
    /// real location; anything else is [`FsContext::check_real`].
    pub(in crate::fs) fn locate(&self, path: &Path, links: EnterLinks) -> Result<PathBuf, Error> {
        if !links.allows_given_links() {
            return self.check_real(path);
        }

        match self.check_real(path) {
            Err(Error::Symlink {
                real: RealPath::Resolved(real),
                ..
            }) => Ok(real),
            checked => checked,
        }
    }

    /// `lstat`s a resolved path after [`FsContext::check_real`] accepts it; a missing path is
    /// an `Io` error naming it.
    pub(super) fn lstat_real(&self, path: &Path) -> Result<Metadata, Error> {
        self.check_real(path)?;
        self.io(Op::Lstat, path, self.backend.lstat(path))
    }

    /// The refusal for an item that is itself a symlink.
    pub(super) fn refuse_symlink_item(&self, path: &Path) -> Error {
        let real = match self.canonical(path) {
            Ok(Canonical::Resolved(real)) => RealPath::Resolved(real),
            Ok(Canonical::Dangling { link, source }) => RealPath::Unresolved(Box::new(Error::Io {
                op: Op::Canonicalize,
                path: link,
                source,
            })),
            Err(error) => RealPath::Unresolved(Box::new(error)),
        };
        Error::Symlink {
            path: path.into(),
            link: path.into(),
            real,
        }
    }

    // canonical form

    /// Canonicalizes the deepest existing ancestor of an absolute path and appends the
    /// missing rest. The first missing name is `lstat`ed: an entry that exists there but
    /// could not be canonicalized is a dangling link.
    fn canonical(&self, path: &Path) -> Result<Canonical, Error> {
        let mut existing = path.to_path_buf();
        let mut missing: Vec<OsString> = Vec::new();
        let real = loop {
            match self.backend.canonicalize(&existing) {
                Ok(real) => break real,
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    let Some(name) = existing.file_name() else {
                        return self.io(Op::Canonicalize, &existing, Err(source));
                    };
                    missing.push(name.to_owned());
                    existing.pop();
                }
                Err(source) => return self.io(Op::Canonicalize, &existing, Err(source)),
            }
        };

        let Some(first_missing) = missing.last() else {
            return Ok(Canonical::Resolved(real));
        };
        let entry = existing.join(first_missing);
        match self.backend.lstat(&entry) {
            Ok(_) => Ok(Canonical::Dangling {
                link: entry,
                source: io::ErrorKind::NotFound.into(),
            }),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                let rest: PathBuf = missing.iter().rev().collect();
                Ok(Canonical::Resolved(real.join(rest)))
            }
            Err(source) => self.io(Op::Lstat, &entry, Err(source)),
        }
    }

    /// The refusal for a path whose canonical form differs: a symlink when one of its
    /// existing components is a link, a misspelling otherwise.
    fn mismatch(&self, path: PathBuf, real: PathBuf) -> Error {
        let mut prefix = PathBuf::from("/");
        for component in path.components() {
            let Component::Normal(name) = component else {
                continue;
            };
            prefix.push(name);
            match self.backend.lstat(&prefix) {
                Ok(meta) if meta.kind == FileKind::Symlink => {
                    let real = RealPath::Resolved(real);
                    return Error::Symlink {
                        path,
                        link: prefix,
                        real,
                    };
                }
                Ok(_) => continue,
                Err(source) if source.kind() == io::ErrorKind::NotFound => break,
                Err(source) => {
                    return Error::Io {
                        op: Op::Lstat,
                        path: prefix,
                        source,
                    };
                }
            }
        }
        Error::Misspelled { typed: path, real }
    }
}
