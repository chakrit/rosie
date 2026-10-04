//! `roots` allowlist operations (`docs/spec/cli.md#management-commands`,
//! `docs/spec/safety.md#roots`).

use std::path::{Path, PathBuf};

use super::{Config, ConfigFile, Error, document, store};
use crate::fs::{Backend, FsContext, FsOp, Paths, UserInput};

/// The current `roots` allowlist, for `rosie roots`.
pub fn list(config: &Config) -> &[PathBuf] {
    &config.roots
}

/// `roots add <path>`: refuses a path with a symlink component, unless the config's
/// `enter_links` allows one, or a spelling that does not match the disk, naming the
/// real path (`docs/spec/safety.md#symlinks`). The entry written is the path as typed,
/// with `.` and `..` resolved, spelled under `~` when the user typed `~` and the path
/// is still under home; a resolved path never replaces it. Adding a root already listed
/// for the same location writes nothing.
pub fn add<B: Backend>(
    ctx: &FsContext<B>,
    config_dir: &Path,
    paths: &Paths,
    file: &ConfigFile,
    path: &Path,
) -> Result<ConfigFile, Error> {
    let located = UserInput::path(path, file.config.walk.enter_links).perform(ctx)?;
    if file.config.roots.contains(&located) {
        return Ok(file.clone());
    }

    let entry = spelled_as_typed(path, &paths.expand(path)?, paths);
    let entry = entry.to_str().ok_or_else(|| Error::RootNotUtf8 {
        path: located.clone(),
    })?;
    let edited = document::with_root(&file.document, entry);

    store(ctx, config_dir, &edited.to_string())
}

/// `roots remove <path>`: `path` is matched with `~` expanded and dots resolved, against
/// each entry as written and as located on disk. A path that is not listed is refused
/// and nothing is written.
pub fn remove<B: Backend>(
    ctx: &FsContext<B>,
    config_dir: &Path,
    paths: &Paths,
    file: &ConfigFile,
    path: &Path,
) -> Result<ConfigFile, Error> {
    let target = paths.expand(path)?;
    let index = file
        .written_roots
        .iter()
        .zip(&file.config.roots)
        .position(|(written, located)| *written == target || *located == target)
        .ok_or(Error::NotARoot { path: target })?;

    let edited = document::without_root(&file.document, index);
    store(ctx, config_dir, &edited.to_string())
}

/// The checked path, written back under `~` when the user typed it that way.
fn spelled_as_typed(typed: &Path, checked: &Path, paths: &Paths) -> PathBuf {
    let typed_under_home = typed.starts_with("~");
    match (typed_under_home, checked.strip_prefix(paths.home())) {
        (true, Ok(rest)) => Path::new("~").join(rest),
        _ => checked.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use crate::fs::fake::FakeBackend;
    use crate::fs::{Bounds, RealPath, Rosie};

    use super::super::load;
    use super::*;

    const HOME: &str = "/Users/me";
    const CONFIG_DIR: &str = "/Users/me/.config/rosie";
    const CONFIG_FILE: &str = "/Users/me/.config/rosie/config.toml";

    fn ctx(fake: &FakeBackend) -> FsContext<&FakeBackend> {
        let bounds = Bounds {
            roots: Vec::new(),
            paths: paths(),
            user_uid: crate::fs::fake::USER_UID,
        };
        FsContext::new(fake, bounds).expect("absolute bounds")
    }

    fn paths() -> Paths {
        Paths::new(Path::new(HOME)).expect("absolute paths")
    }

    fn config_dir() -> &'static Path {
        Path::new(CONFIG_DIR)
    }

    fn loaded(ctx: &FsContext<&FakeBackend>) -> ConfigFile {
        load(ctx, config_dir()).expect("load config")
    }

    fn file_text(ctx: &FsContext<&FakeBackend>) -> String {
        let bytes = Rosie::read(Path::new(CONFIG_FILE))
            .perform(ctx)
            .expect("config file exists");
        String::from_utf8(bytes).expect("config file is UTF-8")
    }

    #[test]
    fn adds_a_root_and_persists_it() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/code");
        fake.add_file(CONFIG_FILE, "roots = []\n");
        let ctx = ctx(&fake);

        let updated = add(
            &ctx,
            config_dir(),
            &paths(),
            &loaded(&ctx),
            Path::new("/Users/me/code"),
        )
        .expect("add a real folder");

        assert_eq!(list(updated.config()), &[PathBuf::from("/Users/me/code")]);
        assert_eq!(loaded(&ctx).config().roots, updated.config().roots);
    }

    #[test]
    fn adding_keeps_comments_and_the_tilde_spelling() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/Applications");
        fake.add_dir("/Users/me/Library/Caches");
        let original = "# my config\nroots = [\n  # apps\n  \"~/Applications\",\n]\n";
        fake.add_file(CONFIG_FILE, original);
        let ctx = ctx(&fake);

        let updated = add(
            &ctx,
            config_dir(),
            &paths(),
            &loaded(&ctx),
            Path::new("~/Library/Caches"),
        )
        .expect("add a tilde path");

        assert_eq!(
            list(updated.config()),
            &[
                PathBuf::from("/Users/me/Applications"),
                PathBuf::from("/Users/me/Library/Caches"),
            ]
        );
        let text = file_text(&ctx);
        assert!(text.contains("# my config"), "{text}");
        assert!(text.contains("# apps"), "{text}");
        assert!(text.contains("\"~/Applications\""), "{text}");
        assert!(text.contains("\"~/Library/Caches\""), "{text}");
    }

    #[test]
    fn refuses_a_path_through_a_symlink_naming_the_real_path() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/elsewhere");
        fake.add_symlink("/Users/me/code", "elsewhere");
        fake.add_file(CONFIG_FILE, "roots = []\n");
        let ctx = ctx(&fake);

        let result = add(
            &ctx,
            config_dir(),
            &paths(),
            &loaded(&ctx),
            Path::new("/Users/me/code"),
        );

        let Err(Error::Fs(crate::fs::Error::Symlink {
            real: RealPath::Resolved(real),
            ..
        })) = result
        else {
            panic!("expected a symlink refusal naming the real path, got {result:?}");
        };
        assert_eq!(real, PathBuf::from("/Users/me/elsewhere"));
    }

    #[test]
    fn refuses_a_spelling_that_does_not_match_the_disk() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/Code");
        fake.add_file(CONFIG_FILE, "roots = []\n");
        let ctx = ctx(&fake);

        let result = add(
            &ctx,
            config_dir(),
            &paths(),
            &loaded(&ctx),
            Path::new("/Users/me/code"),
        );

        let Err(Error::Fs(crate::fs::Error::Misspelled { real, .. })) = result else {
            panic!("expected a spelling refusal, got {result:?}");
        };
        assert_eq!(real, PathBuf::from("/Users/me/Code"));
    }

    // `enter_links = "roots"` (`docs/spec/safety.md#enter_links`)

    const STOW: &str = "[walk]\nenter_links = \"roots\"\n";

    fn stowed(fake: &FakeBackend) {
        fake.add_dir("/Users/me/dotfiles/cargo/registry");
        fake.add_symlink("/Users/me/.cargo", "dotfiles/cargo");
    }

    #[test]
    fn adds_a_root_through_a_symlink_as_written_when_links_are_allowed() {
        let fake = FakeBackend::new();
        stowed(&fake);
        fake.add_file(CONFIG_FILE, format!("roots = []\n{STOW}"));
        let ctx = ctx(&fake);

        let updated = add(
            &ctx,
            config_dir(),
            &paths(),
            &loaded(&ctx),
            Path::new("~/.cargo/registry"),
        )
        .expect("a root through a link");

        assert_eq!(
            list(updated.config()),
            &[PathBuf::from("/Users/me/dotfiles/cargo/registry")]
        );
        let text = file_text(&ctx);
        assert!(text.contains("\"~/.cargo/registry\""), "{text}");
    }

    #[test]
    fn removes_a_root_through_a_symlink_by_its_written_spelling() {
        let fake = FakeBackend::new();
        stowed(&fake);
        fake.add_file(
            CONFIG_FILE,
            format!("roots = [\"~/.cargo/registry\"]\n{STOW}"),
        );
        let ctx = ctx(&fake);

        let updated = remove(
            &ctx,
            config_dir(),
            &paths(),
            &loaded(&ctx),
            Path::new("~/.cargo/registry"),
        )
        .expect("remove the root as written");

        assert_eq!(list(updated.config()), &[] as &[PathBuf]);
    }

    #[test]
    fn adding_an_already_listed_root_writes_nothing() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/code");
        let original = "roots = [ \"/Users/me/code\" ] # mine\n";
        fake.add_file(CONFIG_FILE, original);
        let ctx = ctx(&fake);

        let updated = add(
            &ctx,
            config_dir(),
            &paths(),
            &loaded(&ctx),
            Path::new("/Users/me/code"),
        )
        .expect("re-add the same root");

        assert_eq!(list(updated.config()), &[PathBuf::from("/Users/me/code")]);
        assert_eq!(file_text(&ctx), original);
    }

    #[test]
    fn removes_a_root_by_its_tilde_spelling_keeping_comments() {
        let fake = FakeBackend::new();
        let original = "roots = [\n  \"/Users/me/code\",\n\n  # apps\n  \"~/Applications\",\n  \"~/Library/Caches\",\n]\n";
        fake.add_file(CONFIG_FILE, original);
        let ctx = ctx(&fake);

        let updated = remove(
            &ctx,
            config_dir(),
            &paths(),
            &loaded(&ctx),
            Path::new("~/Applications"),
        )
        .expect("remove a root");

        assert_eq!(
            list(updated.config()),
            &[
                PathBuf::from("/Users/me/code"),
                PathBuf::from("/Users/me/Library/Caches"),
            ]
        );
        let text = file_text(&ctx);
        assert!(!text.contains("~/Applications"), "{text}");
        assert!(text.contains("# apps"), "{text}");
        assert!(text.contains("\"~/Library/Caches\""), "{text}");
        assert_eq!(loaded(&ctx).config().roots, updated.config().roots);
    }

    #[test]
    fn removing_a_path_that_is_not_listed_is_refused_and_writes_nothing() {
        let fake = FakeBackend::new();
        let original = "# mine\nroots = [\"/Users/me/code\"]\n";
        fake.add_file(CONFIG_FILE, original);
        let ctx = ctx(&fake);

        let result = remove(
            &ctx,
            config_dir(),
            &paths(),
            &loaded(&ctx),
            Path::new("/Users/me/other"),
        );

        let Err(Error::NotARoot { path }) = result else {
            panic!("expected a not-a-root refusal, got {result:?}");
        };
        assert_eq!(path, PathBuf::from("/Users/me/other"));
        assert_eq!(file_text(&ctx), original);
    }
}
