//! Absolute paths whose components may hold `*` and `?`: Lua's `glob`
//! (`docs/spec/rules.md#lua`), and `tree <dir>` and app paths on the command line
//! (`docs/spec/cli.md#patterns`).

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use super::backend::{Backend, FileKind};
use super::context::{FsContext, resolve_dots};
use super::error::Error;
use super::ops::{Cleanup, FsOp};
use crate::glob::NameMatch;

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
            .position(|name| has_wildcard(name))
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

    /// The literal folder above the first wildcard, or the whole path when it has none.
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Whether any component holds a wildcard.
    pub fn has_wildcard(&self) -> bool {
        !self.below.is_empty()
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
        let mut found = vec![(base.to_path_buf(), base_kind)];
        for part in &self.below {
            let folders = found
                .into_iter()
                .filter(|&(_, kind)| kind == FileKind::Dir)
                .map(|(folder, _)| folder);
            found = folders
                .map(|folder| part.children(ctx, &folder))
                .collect::<Result<Vec<_>, Error>>()?
                .concat();
        }

        found.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok(found)
    }
}

impl<M: NameMatch> Part<M> {
    fn parse(name: &OsStr) -> Result<Part<M>, Error> {
        if !has_wildcard(name) {
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

        let mut children = Vec::new();
        for child in candidates {
            match Cleanup::lstat(&child).perform(ctx) {
                Ok(meta) if meta.kind == FileKind::Symlink => {}
                Ok(meta) => children.push((child, meta.kind)),
                Err(error) if error.has_vanished() => {}
                Err(error) => return Err(error),
            }
        }
        Ok(children)
    }
}

fn has_wildcard(name: &OsStr) -> bool {
    name.as_bytes()
        .iter()
        .any(|byte| matches!(byte, b'*' | b'?'))
}
