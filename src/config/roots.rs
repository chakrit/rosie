//! `roots` allowlist operations (`docs/spec/cli.md#management-commands`,
//! `docs/spec/safety.md#roots`).

use std::path::{Path, PathBuf};

use super::{Config, ConfigFile, Error, document, expand_home, store};
use crate::fs::{Backend, Gate, Home, resolve_dots};

/// The current `roots` allowlist, for `rosie roots`.
pub fn list(config: &Config) -> &[PathBuf] {
    &config.roots
}

/// `roots add <path>`: refuses a path with a symlink component or a spelling that does
/// not match the disk, naming the real path (`docs/spec/safety.md#symlinks`). The entry
/// written is the checked path, with `.` and `..` resolved, spelled under `~` when the
/// user typed `~` and the path is still under home. Adding an already-listed root writes
/// nothing.
pub fn add<B: Backend>(
    gate: &Gate<B>,
    config_dir: &Path,
    home: &Home,
    file: &ConfigFile,
    path: &Path,
) -> Result<ConfigFile, Error> {
    let checked = gate.check_typed_path(&expand_home(path, home))?;
    if file.config.roots.contains(&checked) {
        return Ok(file.clone());
    }

    let entry = spelled_as_typed(path, &checked, home);
    let entry = entry.to_str().ok_or_else(|| Error::RootNotUtf8 {
        path: checked.clone(),
    })?;
    let edited = document::with_root(&file.document, entry);

    store(gate, config_dir, home, &edited.to_string())
}

/// `roots remove <path>`: `path` is matched the way entries are read, with `~` expanded
/// and dots resolved. A path that is not listed is refused and nothing is written.
pub fn remove<B: Backend>(
    gate: &Gate<B>,
    config_dir: &Path,
    home: &Home,
    file: &ConfigFile,
    path: &Path,
) -> Result<ConfigFile, Error> {
    let target = resolve_dots(&expand_home(path, home))?;
    let index = file
        .config
        .roots
        .iter()
        .position(|root| *root == target)
        .ok_or(Error::NotARoot { path: target })?;

    let edited = document::without_root(&file.document, index);
    store(gate, config_dir, home, &edited.to_string())
}

/// The checked path, written back under `~` when the user typed it that way.
fn spelled_as_typed(typed: &Path, checked: &Path, home: &Home) -> PathBuf {
    let typed_under_home = typed.starts_with("~");
    match (typed_under_home, checked.strip_prefix(home)) {
        (true, Ok(rest)) => Path::new("~").join(rest),
        _ => checked.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use crate::fs::fake::FakeBackend;
    use crate::fs::{Bounds, RealPath};

    use super::super::load;
    use super::*;

    const HOME: &str = "/Users/me";
    const CONFIG_DIR: &str = "/Users/me/.config/rosie";
    const CONFIG_FILE: &str = "/Users/me/.config/rosie/config.toml";
    const DATA_DIR: &str = "/Users/me/.local/share/rosie";

    fn gate(fake: &FakeBackend) -> Gate<&FakeBackend> {
        let bounds = Bounds {
            roots: Vec::new(),
            config_dir: PathBuf::from(CONFIG_DIR),
            data_dir: PathBuf::from(DATA_DIR),
            user_uid: crate::fs::fake::USER_UID,
        };
        Gate::new(fake, bounds).expect("absolute bounds")
    }

    fn home() -> Home {
        Home::new(Path::new(HOME)).expect("absolute home")
    }

    fn config_dir() -> &'static Path {
        Path::new(CONFIG_DIR)
    }

    fn loaded(gate: &Gate<&FakeBackend>) -> ConfigFile {
        load(gate, config_dir(), &home()).expect("load config")
    }

    fn file_text(gate: &Gate<&FakeBackend>) -> String {
        let bytes = gate
            .read_own_file(Path::new(CONFIG_FILE))
            .expect("config file exists");
        String::from_utf8(bytes).expect("config file is UTF-8")
    }

    #[test]
    fn adds_a_root_and_persists_it() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/code");
        fake.add_file(CONFIG_FILE, "roots = []\n");
        let gate = gate(&fake);

        let updated = add(
            &gate,
            config_dir(),
            &home(),
            &loaded(&gate),
            Path::new("/Users/me/code"),
        )
        .expect("add a real folder");

        assert_eq!(list(updated.config()), &[PathBuf::from("/Users/me/code")]);
        assert_eq!(loaded(&gate).config().roots, updated.config().roots);
    }

    #[test]
    fn adding_keeps_comments_and_the_tilde_spelling() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/Applications");
        fake.add_dir("/Users/me/Library/Caches");
        let original = "# my config\nroots = [\n  # apps\n  \"~/Applications\",\n]\n";
        fake.add_file(CONFIG_FILE, original);
        let gate = gate(&fake);

        let updated = add(
            &gate,
            config_dir(),
            &home(),
            &loaded(&gate),
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
        let text = file_text(&gate);
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
        let gate = gate(&fake);

        let result = add(
            &gate,
            config_dir(),
            &home(),
            &loaded(&gate),
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
        let gate = gate(&fake);

        let result = add(
            &gate,
            config_dir(),
            &home(),
            &loaded(&gate),
            Path::new("/Users/me/code"),
        );

        let Err(Error::Fs(crate::fs::Error::Misspelled { real, .. })) = result else {
            panic!("expected a spelling refusal, got {result:?}");
        };
        assert_eq!(real, PathBuf::from("/Users/me/Code"));
    }

    #[test]
    fn adding_an_already_listed_root_writes_nothing() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/code");
        let original = "roots = [ \"/Users/me/code\" ] # mine\n";
        fake.add_file(CONFIG_FILE, original);
        let gate = gate(&fake);

        let updated = add(
            &gate,
            config_dir(),
            &home(),
            &loaded(&gate),
            Path::new("/Users/me/code"),
        )
        .expect("re-add the same root");

        assert_eq!(list(updated.config()), &[PathBuf::from("/Users/me/code")]);
        assert_eq!(file_text(&gate), original);
    }

    #[test]
    fn removes_a_root_by_its_tilde_spelling_keeping_comments() {
        let fake = FakeBackend::new();
        let original = "roots = [\n  \"/Users/me/code\",\n\n  # apps\n  \"~/Applications\",\n  \"~/Library/Caches\",\n]\n";
        fake.add_file(CONFIG_FILE, original);
        let gate = gate(&fake);

        let updated = remove(
            &gate,
            config_dir(),
            &home(),
            &loaded(&gate),
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
        let text = file_text(&gate);
        assert!(!text.contains("~/Applications"), "{text}");
        assert!(text.contains("# apps"), "{text}");
        assert!(text.contains("\"~/Library/Caches\""), "{text}");
        assert_eq!(loaded(&gate).config().roots, updated.config().roots);
    }

    #[test]
    fn removing_a_path_that_is_not_listed_is_refused_and_writes_nothing() {
        let fake = FakeBackend::new();
        let original = "# mine\nroots = [\"/Users/me/code\"]\n";
        fake.add_file(CONFIG_FILE, original);
        let gate = gate(&fake);

        let result = remove(
            &gate,
            config_dir(),
            &home(),
            &loaded(&gate),
            Path::new("/Users/me/other"),
        );

        let Err(Error::NotARoot { path }) = result else {
            panic!("expected a not-a-root refusal, got {result:?}");
        };
        assert_eq!(path, PathBuf::from("/Users/me/other"));
        assert_eq!(file_text(&gate), original);
    }
}
