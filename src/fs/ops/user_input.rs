//! Ops on paths and files the user names: a path typed on the command line or written in
//! `roots`, and a plan file (`docs/spec/safety.md#symlinks`).

use std::path::{Path, PathBuf};

use super::{FsOp, sealed};
use crate::fs::backend::{Backend, Metadata};
use crate::fs::context::FsContext;
use crate::fs::error::Error;

/// The owner of every op on what the user names.
pub enum UserInput {}

impl UserInput {
    /// Makes a written path absolute and accepts it only as its own canonical form on
    /// disk: `~` expanded against the injected home, `.`, `..`, and repeated `/`
    /// resolved by text, then any difference from the canonical form refused as a
    /// symlink or a misspelling, naming the canonical path. A missing path is checked
    /// up to its deepest existing ancestor.
    pub fn path(path: &Path) -> Resolve<'_> {
        Resolve { path }
    }

    /// Reads a file the user named, such as a plan, following any symlink on its way.
    pub fn read(path: &Path) -> Read<'_> {
        Read { path }
    }

    /// `lstat`s a path the user's environment names, such as the home folder.
    pub fn lstat(path: &Path) -> Lstat<'_> {
        Lstat { path }
    }
}

pub struct Resolve<'p> {
    path: &'p Path,
}

pub struct Read<'p> {
    path: &'p Path,
}

pub struct Lstat<'p> {
    path: &'p Path,
}

impl sealed::Sealed for Resolve<'_> {}
impl sealed::Sealed for Read<'_> {}
impl sealed::Sealed for Lstat<'_> {}

impl<B: Backend> FsOp<B> for Resolve<'_> {
    type Output = PathBuf;

    fn perform(self, ctx: &FsContext<B>) -> Result<PathBuf, Error> {
        let expanded = ctx.paths().expand(self.path)?;
        ctx.check_real(&expanded)
    }
}

impl<B: Backend> FsOp<B> for Read<'_> {
    type Output = Vec<u8>;

    fn perform(self, ctx: &FsContext<B>) -> Result<Vec<u8>, Error> {
        ctx.read_file_through_links(self.path)
    }
}

impl<B: Backend> FsOp<B> for Lstat<'_> {
    type Output = Metadata;

    fn perform(self, ctx: &FsContext<B>) -> Result<Metadata, Error> {
        ctx.lstat(self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::RealPath;
    use crate::fs::fake::FakeBackend;
    use crate::fs::paths::Paths;

    fn ctx(fake: &FakeBackend) -> FsContext<&FakeBackend> {
        let paths = Paths::new(Path::new("/Users/me")).expect("absolute home");
        FsContext::new(fake, paths.bounds(Vec::new(), 501)).expect("absolute bounds")
    }

    #[test]
    fn resolves_a_home_path_that_is_its_own_canonical_form() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/Library/Caches");

        let resolved = UserInput::path(Path::new("~//Library/Caches/gone")).perform(&ctx(&fake));

        assert_eq!(
            resolved.expect("real path"),
            PathBuf::from("/Users/me/Library/Caches/gone")
        );
    }

    #[test]
    fn refuses_a_missing_path_below_a_symlinked_ancestor() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/dotfiles/cargo");
        fake.add_symlink("/Users/me/.cargo", "dotfiles/cargo");

        let resolved = UserInput::path(Path::new("~/.cargo/registry")).perform(&ctx(&fake));

        let Err(Error::Symlink { link, real, .. }) = resolved else {
            panic!("expected a symlink refusal, got {resolved:?}");
        };
        assert_eq!(link, PathBuf::from("/Users/me/.cargo"));
        assert!(
            matches!(real, RealPath::Resolved(real) if real == Path::new("/Users/me/dotfiles/cargo/registry"))
        );
    }

    #[test]
    fn refuses_a_misspelling_naming_the_spelling_on_disk() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/Library/Caches");

        let resolved = UserInput::path(Path::new("~/library/caches/x")).perform(&ctx(&fake));

        let Err(Error::Misspelled { real, .. }) = resolved else {
            panic!("expected a spelling refusal, got {resolved:?}");
        };
        assert_eq!(real, PathBuf::from("/Users/me/Library/Caches/x"));
    }

    // `rosie run /tmp/plan.toml` reads the plan through macOS's `/tmp` link.
    #[test]
    fn reads_a_named_file_through_a_symlink() {
        let fake = FakeBackend::new();
        fake.add_file("/private/tmp/plan.toml", "version = 1\n");
        fake.add_symlink("/tmp", "private/tmp");

        let read = UserInput::read(Path::new("/tmp/plan.toml")).perform(&ctx(&fake));

        assert_eq!(read.expect("plan read through the link"), b"version = 1\n");
    }
}
