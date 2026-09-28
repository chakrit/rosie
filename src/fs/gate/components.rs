//! Component-by-component checks of a path against the disk: the symlink ban and the
//! spelling check for user-typed paths (`docs/spec/safety.md#symlinks`).

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Component, Path, PathBuf};

use super::{Gate, resolve_dots};
use crate::fs::backend::{Backend, FileKind, Metadata};
use crate::fs::error::{Error, Op, RealPath};

/// The most symlinks followed while naming a real path, as the kernel's `MAXSYMLINKS`.
const MAX_SYMLINK_HOPS: usize = 32;

/// What walking a path component by component from `/` found.
enum Walk {
    /// No component is a symlink. `real` is the path with `..` applied; `meta` is its
    /// own metadata.
    Clear { real: PathBuf, meta: Metadata },
    /// `missing` does not exist, so nothing from there down can be a link. `real` is
    /// the whole path with `..` applied.
    Missing {
        real: PathBuf,
        missing: PathBuf,
        source: io::Error,
    },
    /// `link` is the first symlink; `rest` is what the path continues with after it.
    Symlink { link: PathBuf, rest: PathBuf },
}

impl<B: Backend> Gate<B> {
    /// Checks a path the user typed (`roots add`, a scan folder, an app argument) and
    /// returns it with `.` and `..` resolved.
    ///
    /// Each component is `lstat`ed and compared with its parent's listing in one pass. A
    /// symlink is refused, naming the path through the link's target. A component whose
    /// case or Unicode form differs from the disk is refused, naming the spelling on
    /// disk; nothing is case-folded.
    pub fn check_typed_path(&self, path: &Path) -> Result<PathBuf, Error> {
        let typed = resolve_dots(path)?;

        let mut real = PathBuf::from("/");
        for name in normal_names(&typed) {
            let typed_here = real.join(name);
            let meta = self.io(Op::Lstat, &typed_here, self.backend.lstat(&typed_here))?;
            let real_here = real.join(self.spelling_on_disk(&real, name, meta)?);

            if meta.kind == FileKind::Symlink {
                let rest = remaining_after(&typed, real_here.components().count());
                return Err(self.refuse_symlink(&typed, &real_here, &rest));
            }
            real = real_here;
        }

        match real == typed {
            true => Ok(typed),
            false => Err(Error::Misspelled { typed, real }),
        }
    }

    /// Checks a `roots` config entry: refused only when an existing path component is a
    /// symlink. A path, or the tail of one, that does not exist on disk is valid — a
    /// seeded tool-cache root is absent on a machine without that tool
    /// (`docs/spec/safety.md#roots`).
    pub fn check_root_entry(&self, path: &Path) -> Result<PathBuf, Error> {
        let path = resolve_dots(path)?;
        self.refuse_symlinks_in_existing(&path)?;
        Ok(path)
    }

    /// The name `name` is stored under in `folder`, as `folder`'s listing spells it.
    ///
    /// The volume looks `name` up as it compares names, so on a volume that folds case
    /// or Unicode form, as APFS does by default, this is the entry `name` folds onto.
    /// Only the volume knows its folding rules, so rosie asks it rather than folding
    /// names itself. A missing entry is an [`Error::Io`] of kind `NotFound`; a `name`
    /// that is not one entry's name, such as `..` or one holding `/`, is refused.
    pub fn stored_name(&self, folder: &Path, name: &OsStr) -> Result<OsString, Error> {
        if Path::new(name).file_name() != Some(name) {
            return Err(Error::NotAnEntryName {
                name: name.to_owned(),
            });
        }

        let folder = resolve_dots(folder)?;
        let path = folder.join(name);

        let meta = self.io(Op::Lstat, &path, self.backend.lstat(&path))?;
        self.spelling_on_disk(&folder, name, meta)
    }

    /// `lstat`s every component of an already resolved, existing path from `/` down,
    /// refusing the first symlink. Returns the metadata of the path itself.
    pub(super) fn lstat_without_symlinks(&self, path: &Path) -> Result<Metadata, Error> {
        match self.walk_components(path)? {
            Walk::Clear { meta, .. } => Ok(meta),
            Walk::Missing {
                missing, source, ..
            } => Err(Error::Io {
                op: Op::Lstat,
                path: missing,
                source,
            }),
            Walk::Symlink { link, rest } => Err(self.refuse_symlink(path, &link, &rest)),
        }
    }

    /// Refuses a symlink in any existing component of an already resolved path that may
    /// not exist yet, such as a file about to be written. Missing components cannot be
    /// links.
    pub(super) fn refuse_symlinks_in_existing(&self, path: &Path) -> Result<(), Error> {
        match self.walk_components(path)? {
            Walk::Clear { .. } | Walk::Missing { .. } => Ok(()),
            Walk::Symlink { link, rest } => Err(self.refuse_symlink(path, &link, &rest)),
        }
    }

    /// Builds the refusal for `path`, which reaches the symlink `link` and continues with
    /// `rest`, naming the real path when it can be found.
    pub(super) fn refuse_symlink(&self, path: &Path, link: &Path, rest: &Path) -> Error {
        let real = match self.chase_links(link, rest) {
            Ok(real) => RealPath::Resolved(real),
            Err(error) => RealPath::Unresolved(Box::new(error)),
        };
        Error::Symlink {
            path: path.into(),
            link: link.into(),
            real,
        }
    }

    // walking

    /// The path through `link` and on with `rest`, with every symlink along it replaced
    /// by its target until none is left, following at most [`MAX_SYMLINK_HOPS`] links.
    fn chase_links(&self, link: &Path, rest: &Path) -> Result<PathBuf, Error> {
        let mut link = link.to_path_buf();
        let mut rest = rest.to_path_buf();
        for _ in 0..MAX_SYMLINK_HOPS {
            let candidate = self.through_target(&link, &rest)?;
            match self.walk_components(&candidate)? {
                Walk::Clear { real, .. } | Walk::Missing { real, .. } => return Ok(real),
                Walk::Symlink {
                    link: next,
                    rest: after,
                } => (link, rest) = (next, after),
            }
        }

        let source = io::Error::other("too many levels of symbolic links");
        Err(Error::Io {
            op: Op::ReadLink,
            path: link,
            source,
        })
    }

    /// The path that replaces `link` with its stored target, continued with `rest`.
    /// A relative target is taken from the link's folder, which the walk found real.
    fn through_target(&self, link: &Path, rest: &Path) -> Result<PathBuf, Error> {
        let target = self.io(Op::ReadLink, link, self.backend.read_link(link))?;
        let Some(parent) = link.parent() else {
            let source = io::Error::from(io::ErrorKind::InvalidInput);
            return Err(Error::Io {
                op: Op::ReadLink,
                path: link.into(),
                source,
            });
        };
        Ok(parent.join(target).join(rest))
    }

    /// `lstat`s each component of an absolute path from `/` down and stops at the first
    /// symlink or missing entry. `..` steps back up the real folder reached so far, as
    /// the kernel does; the gate's own paths have none left.
    fn walk_components(&self, path: &Path) -> Result<Walk, Error> {
        let components: Vec<Component> = path.components().collect();
        let mut current = PathBuf::from("/");
        let mut meta = self.io(Op::Lstat, &current, self.backend.lstat(&current))?;

        for (index, component) in components.iter().enumerate() {
            match component {
                Component::Normal(name) => current.push(name),
                Component::ParentDir => {
                    current.pop();
                }
                Component::RootDir | Component::CurDir | Component::Prefix(_) => continue,
            }
            let rest = || components[index + 1..].iter().collect::<PathBuf>();

            meta = match self.backend.lstat(&current) {
                Ok(meta) => meta,
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    let real = resolve_dots(&current.join(rest()))?;
                    let missing = current;
                    return Ok(Walk::Missing {
                        real,
                        missing,
                        source,
                    });
                }
                Err(source) => return self.io(Op::Lstat, &current, Err(source)),
            };
            if meta.kind == FileKind::Symlink {
                let link = current;
                return Ok(Walk::Symlink { link, rest: rest() });
            }
        }
        Ok(Walk::Clear {
            real: current,
            meta,
        })
    }

    /// The name of the entry `name` refers to, as spelled in `parent`'s listing. When the
    /// typed name is not listed verbatim, the entry is found by its dev and inode, so no
    /// case folding or Unicode normalization is needed.
    fn spelling_on_disk(
        &self,
        parent: &Path,
        name: &OsStr,
        meta: Metadata,
    ) -> Result<OsString, Error> {
        let listing = self.io(Op::ReadDir, parent, self.backend.read_dir(parent))?;
        if listing.iter().any(|entry| entry == name) {
            return Ok(name.to_owned());
        }

        for entry in listing {
            let sibling = parent.join(&entry);
            let sibling_meta = self.io(Op::Lstat, &sibling, self.backend.lstat(&sibling))?;
            if (sibling_meta.dev, sibling_meta.inode) == (meta.dev, meta.inode) {
                return Ok(entry);
            }
        }

        let source = io::Error::other("the entry is missing from its folder's listing");
        Err(Error::Io {
            op: Op::ReadDir,
            path: parent.join(name),
            source,
        })
    }
}

/// The named components of a resolved path, which holds only a root and names.
fn normal_names(path: &Path) -> impl Iterator<Item = &OsStr> {
    path.components().filter_map(|component| match component {
        Component::Normal(name) => Some(name),
        _ => None,
    })
}

/// The components of `path` after its first `count` components.
fn remaining_after(path: &Path, count: usize) -> PathBuf {
    path.components().skip(count).collect()
}
