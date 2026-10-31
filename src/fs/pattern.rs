//! Absolute paths whose components may hold `*` and `?`: Lua's `glob`
//! (`docs/spec/rules.md#lua`), and `tree <dir>` and app paths on the command line
//! (`docs/spec/cli.md#patterns`).

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use super::backend::{Backend, FileKind};
use super::context::FsContext;
use super::error::Error;
use super::links::EnterLinks;
use super::ops::{Cleanup, FsOp, UserInput};
use crate::wildcard::{NameMatch, is_glob};

/// A path pattern split at its first wildcard component: the literal folder above it,
/// which the caller checks under its own policy, and the components from there down.
pub struct PathPattern<M> {
    /// The path as given, made absolute.
    path: PathBuf,
    base: PathBuf,
    below: Vec<Part<M>>,
}

/// One component below the base.
enum Part<M> {
    Literal(OsString),
    Wildcard(M),
    /// A `..` below the first wildcard: the folder holding each match, as a shell
    /// resolves it.
    Parent,
}

impl<M: NameMatch> PathPattern<M> {
    /// Parses an absolute path. A `..` above the first wildcard resolves in the text; one
    /// below it climbs from each match.
    pub fn parse(path: &Path) -> Result<PathPattern<M>, Error> {
        if !path.has_root() {
            return Err(Error::Relative {
                path: path.to_path_buf(),
            });
        }

        PathPattern::typed_under(Path::new("/"), path)
    }

    /// Parses `typed`, a path the user typed, taken under `folder` when it is relative,
    /// `..` taken as [`PathPattern::parse`] takes it. Only `typed`'s own components may
    /// hold wildcards: `folder` is literal however it is spelled.
    pub fn typed_under(folder: &Path, typed: &Path) -> Result<PathPattern<M>, Error> {
        let path = folder.join(typed);
        if !path.has_root() {
            return Err(Error::Relative { path });
        }
        let anchor = match typed.has_root() {
            true => Path::new("/"),
            false => folder,
        };
        let mut components = anchor
            .components()
            .map(|component| (component, false))
            .chain(typed.components().map(|component| (component, true)));

        let (names, below) = components.try_fold(
            (Vec::new(), Vec::new()),
            |(mut names, mut below), (component, from_typed)| {
                match (component, below.is_empty()) {
                    (Component::Normal(name), true)
                        if !(from_typed && is_glob(name.as_bytes())) =>
                    {
                        names.push(name)
                    }
                    (Component::Normal(name), _) => below.push(Part::parse(name)?),
                    (Component::ParentDir, true) => drop(names.pop()),
                    (Component::ParentDir, false) => below.push(Part::Parent),
                    (Component::RootDir, _) => names.clear(),
                    (Component::CurDir | Component::Prefix(_), _) => {}
                }
                Ok::<_, Error>((names, below))
            },
        )?;
        let base: PathBuf = std::iter::once(OsStr::new("/")).chain(names).collect();

        Ok(PathPattern { path, base, below })
    }

    /// Checks every wildcard component of `path`, relative or not, as
    /// [`PathPattern::parse`] parses it, so a path the user typed is refused where it
    /// enters.
    pub fn check(path: &Path) -> Result<(), Error> {
        path.iter()
            .filter(|name| is_glob(name.as_bytes()))
            .try_for_each(|name| Part::<M>::parse(name).map(drop))
    }

    /// The path as given, made absolute, `..` unresolved.
    pub fn path(&self) -> &Path {
        &self.path
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

        self.below.iter().try_fold(start, |found, part| {
            let folders = found
                .into_iter()
                .filter(|&(_, kind)| kind == FileKind::Dir)
                .map(|(folder, _)| folder);
            let mut children = folders
                .map(|folder| part.children(ctx, &folder))
                .collect::<Result<Vec<_>, Error>>()?
                .concat();
            children.sort_by(|(a, _), (b, _)| a.cmp(b));
            children.dedup_by(|(a, _), (b, _)| a == b);
            Ok(children)
        })
    }
}

impl<M: NameMatch> Part<M> {
    fn parse(name: &OsStr) -> Result<Part<M>, Error> {
        if !is_glob(name.as_bytes()) {
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
            Part::Parent => vec![folder.parent().unwrap_or(folder).to_path_buf()],
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wildcard::Pattern;

    fn typed(folder: &str, typed: &str) -> PathPattern<Pattern> {
        PathPattern::typed_under(Path::new(folder), Path::new(typed)).expect("parses")
    }

    #[test]
    fn the_folder_a_relative_path_is_under_is_literal() {
        let here = typed("/Users/me/what?", ".");
        let below = typed("/Users/me/what?", "web-*");

        assert!(!here.has_wildcard());
        assert_eq!(here.base(), Path::new("/Users/me/what?"));
        assert!(below.has_wildcard());
        assert_eq!(below.base(), Path::new("/Users/me/what?"));
    }

    #[test]
    fn an_absolute_typed_path_ignores_the_folder() {
        let pattern = typed("/Users/me/what?", "/Users/me/code/*");

        assert_eq!(pattern.base(), Path::new("/Users/me/code"));
        assert_eq!(pattern.path(), Path::new("/Users/me/code/*"));
    }

    #[test]
    fn dot_dot_climbs_out_of_the_folder_before_matching() {
        let pattern = typed("/Users/me/code/app", "../web-*");

        assert_eq!(pattern.base(), Path::new("/Users/me/code"));
        assert!(pattern.has_wildcard());
    }

    #[test]
    fn refuses_a_relative_folder() {
        let result = PathPattern::<Pattern>::typed_under(Path::new("rel"), Path::new("x"));

        assert!(matches!(result, Err(Error::Relative { .. })));
    }
}
