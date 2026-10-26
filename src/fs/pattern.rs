//! Absolute paths whose components may hold `*` and `?`: Lua's `glob`
//! (`docs/spec/rules.md#lua`), and `tree <dir>` and app paths on the command line
//! (`docs/spec/cli.md#patterns`).

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use super::backend::{Backend, FileKind};
use super::context::{FsContext, resolve_dots};
use super::error::Error;
use super::links::EnterLinks;
use super::ops::{Cleanup, FsOp, UserInput};
use crate::glob::{NameMatch, has_wildcard};

/// A path pattern split at its first wildcard component: the literal folder above it,
/// which the caller checks under its own policy, and the components from there down.
pub struct PathPattern<M> {
    base: PathBuf,
    below: Vec<Part<M>>,
}

/// One component below the base.
enum Part<M> {
    Literal(OsString),
    Wildcard(M),
}

impl<M: NameMatch> PathPattern<M> {
    /// Parses an absolute path, `..` resolved first.
    pub fn parse(path: &Path) -> Result<PathPattern<M>, Error> {
        let resolved = resolve_dots(path)?;
        let names: Vec<&OsStr> = resolved
            .components()
            .filter_map(|component| match component {
                Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect();
        let literal_len = names
            .iter()
            .position(|name| has_wildcard(name.as_bytes()))
            .unwrap_or(names.len());

        let base: PathBuf = std::iter::once(OsStr::new("/"))
            .chain(names[..literal_len].iter().copied())
            .collect();
        let below = names[literal_len..]
            .iter()
            .map(|name| Part::parse(name))
            .collect::<Result<_, _>>()?;
        Ok(PathPattern { base, below })
    }

    /// Checks every wildcard component of `path`, relative or not, as
    /// [`PathPattern::parse`] parses it, so a path the user typed is refused where it
    /// enters.
    pub fn check(path: &Path) -> Result<(), Error> {
        path.iter()
            .filter(|name| has_wildcard(name.as_bytes()))
            .try_for_each(|name| Part::<M>::parse(name).map(drop))
    }

    /// The literal folder above the first wildcard, or the whole path when it has none.
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Whether any component holds a wildcard.
    pub fn has_wildcard(&self) -> bool {
        !self.below.is_empty()
    }

    /// The entries a pattern the user typed names, sorted by path. The literal folder
    /// above its first wildcard passes the typed-path checks under `links`; when it does
    /// not exist, nothing matches.
    pub fn expand_typed<B: Backend>(
        &self,
        ctx: &FsContext<B>,
        links: EnterLinks,
    ) -> Result<Vec<(PathBuf, FileKind)>, Error> {
        let base = match UserInput::path(&self.base, links).perform(ctx) {
            Ok(base) => base,
            Err(error) if error.has_vanished() => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let base_meta = match Cleanup::lstat(&base).perform(ctx) {
            Ok(meta) => meta,
            Err(error) if error.has_vanished() => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };

        self.expand(ctx, &base, base_meta.kind)
    }

    /// The entries the pattern names, sorted by path, below `base`: where the caller
    /// found [`PathPattern::base`] on disk, of kind `base_kind`. Symlinks below it are
    /// neither matched nor descended.
    pub fn expand<B: Backend>(
        &self,
        ctx: &FsContext<B>,
        base: &Path,
        base_kind: FileKind,
    ) -> Result<Vec<(PathBuf, FileKind)>, Error> {
        let start = vec![(base.to_path_buf(), base_kind)];
        let mut found = self.below.iter().try_fold(start, |found, part| {
            let folders = found
                .into_iter()
                .filter(|&(_, kind)| kind == FileKind::Dir)
                .map(|(folder, _)| folder);
            folders
                .map(|folder| part.children(ctx, &folder))
                .collect::<Result<Vec<_>, Error>>()
                .map(|children| children.concat())
        })?;

        found.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok(found)
    }
}

impl<M: NameMatch> Part<M> {
    fn parse(name: &OsStr) -> Result<Part<M>, Error> {
        if !has_wildcard(name.as_bytes()) {
            return Ok(Part::Literal(name.to_os_string()));
        }

        let Some(text) = name.to_str() else {
            return Err(Error::PatternNotUtf8 {
                part: name.to_os_string(),
            });
        };
        Ok(Part::Wildcard(M::parse(text)?))
    }

    /// The entries of `folder`, a link-free folder, that this component names, without
    /// symlinks.
    fn children<B: Backend>(
        &self,
        ctx: &FsContext<B>,
        folder: &Path,
    ) -> Result<Vec<(PathBuf, FileKind)>, Error> {
        let candidates = match self {
            Part::Literal(name) => vec![folder.join(name)],
            Part::Wildcard(matcher) => Cleanup::read_dir(folder)
                .perform(ctx)?
                .into_iter()
                .filter(|entry| {
                    entry
                        .name
                        .to_str()
                        .is_some_and(|name| matcher.matches(name))
                })
                .map(|entry| folder.join(entry.name))
                .collect(),
        };

        candidates
            .into_iter()
            .filter_map(|child| match Cleanup::lstat(&child).perform(ctx) {
                Ok(meta) if meta.kind == FileKind::Symlink => None,
                Ok(meta) => Some(Ok((child, meta.kind))),
                Err(error) if error.has_vanished() => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }
}
