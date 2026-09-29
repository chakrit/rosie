//! User configuration: `~/.config/rosie/config.toml`
//! (`docs/spec/cli.md#configuration`).
//!
//! All reads and writes go through the gate's own-data methods
//! (`docs/spec/safety.md#rosies-own-data`); nothing here reads `$HOME` or the
//! environment directly. `home` and `config_dir` are always injected by the caller.
//!
//! The file is kept as an editable TOML document next to its typed reading, so every
//! change edits only the key it names: the user's comments, spelling (`~/…`), and layout
//! survive.

mod document;
pub mod keys;
pub mod roots;

use std::io;
use std::path::{Path, PathBuf};
use std::str::Utf8Error;

use serde::Deserialize;
use thiserror::Error;
use toml_edit::DocumentMut;

use crate::fs::{self, Backend, Gate, Home};

pub use keys::{Key, WalkFlag};

const FILE_NAME: &str = "config.toml";

/// What an error in the default text that seeds `config.toml` names as its file.
const DEFAULT_LABEL: &str = "<default config.toml>";

/// The user's config, typed. Fields default when the key is absent, so an empty or
/// partial file is valid. A key rosie does not know is a load error.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub roots: Vec<PathBuf>,
    pub walk: Walk,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Walk {
    pub enter_bundles: bool,
    pub enter_mounts: bool,
    pub enter_placeholders: bool,
}

impl Walk {
    pub fn flag(self, flag: WalkFlag) -> bool {
        match flag {
            WalkFlag::EnterBundles => self.enter_bundles,
            WalkFlag::EnterMounts => self.enter_mounts,
            WalkFlag::EnterPlaceholders => self.enter_placeholders,
        }
    }
}

/// The user's config file as loaded: the document as written, and its validated typed
/// reading with every root expanded and checked. `config.roots[i]` is the reading of the
/// `i`th entry of the document's `roots` array.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    document: DocumentMut,
    config: Config,
}

impl ConfigFile {
    pub fn config(&self) -> &Config {
        &self.config
    }
}

#[derive(Debug, Error)]
pub enum Error {
    /// No user config file exists yet; the caller seeds one (`load_or_seed`).
    #[error("no config file yet")]
    NotFound,

    #[error(transparent)]
    Fs(#[from] fs::Error),

    #[error("{path} is not valid UTF-8: {source}", path = path.display())]
    NotUtf8 {
        path: PathBuf,
        #[source]
        source: Utf8Error,
    },

    #[error("{path} has an invalid config: {source}", path = path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml_edit::de::Error,
    },

    #[error("the default config is invalid: {0}")]
    InvalidDefault(#[source] Box<Error>),

    #[error("{name} is not a config key")]
    UnknownKey { name: String },

    #[error("roots is not set through `config`; use `rosie roots add` or `rosie roots remove`")]
    RootsNotSettable,

    #[error("{key} does not accept `{value}`; use true or false")]
    InvalidValue { key: &'static str, value: String },

    #[error("{path} is not one of the roots", path = path.display())]
    NotARoot { path: PathBuf },

    #[error("{path} cannot be stored in config.toml: it is not valid UTF-8", path = path.display())]
    RootNotUtf8 { path: PathBuf },

    #[error(
        "{path} has an invalid config: `{key}` must be {expected}",
        path = path.display()
    )]
    InvalidShape {
        path: PathBuf,
        key: &'static str,
        expected: &'static str,
    },

    #[error(
        "{path}: \"{a}\" and \"{b}\" are both {resolved}; keep one",
        path = path.display(),
        a = a.display(),
        b = b.display(),
        resolved = resolved.display()
    )]
    DuplicateRoot {
        path: PathBuf,
        a: PathBuf,
        b: PathBuf,
        resolved: PathBuf,
    },
}

// loading and seeding

/// Loads the user's config, seeding it from `default_text` first when none exists yet
/// (`docs/spec/safety.md#roots`: a fresh `config.toml` is seeded with ordinary,
/// editable `roots` entries). The caller supplies the seed text.
pub fn load_or_seed<B: Backend>(
    gate: &Gate<B>,
    config_dir: &Path,
    home: &Home,
    default_text: &str,
) -> Result<ConfigFile, Error> {
    match load(gate, config_dir, home) {
        Err(Error::NotFound) => seed(gate, config_dir, home, default_text),
        loaded => loaded,
    }
}

/// Loads and validates the user's config file. Returns [`Error::NotFound`] when none
/// exists yet.
pub fn load<B: Backend>(
    gate: &Gate<B>,
    config_dir: &Path,
    home: &Home,
) -> Result<ConfigFile, Error> {
    let path = config_path(config_dir);
    let raw = gate.read_own_file(&path).map_err(load_error)?;
    let text = str::from_utf8(&raw).map_err(|source| Error::NotUtf8 {
        path: path.clone(),
        source,
    })?;
    parse(text, &path, gate, home)
}

/// Writes `default_text` as the user's config file, first validating it the same way a
/// loaded file is validated, then returns it loaded. A validation failure is
/// [`Error::InvalidDefault`]; where it names the config file, it names
/// [`DEFAULT_LABEL`], never the user's config path, which is not written yet.
/// Private: `load_or_seed` is the only public seeding path, so a caller can never
/// overwrite an existing user config.
fn seed<B: Backend>(
    gate: &Gate<B>,
    config_dir: &Path,
    home: &Home,
    default_text: &str,
) -> Result<ConfigFile, Error> {
    let path = config_path(config_dir);
    let file = parse(default_text, Path::new(DEFAULT_LABEL), gate, home)
        .map_err(|source| Error::InvalidDefault(Box::new(source)))?;
    write_validated(gate, config_dir, &path, default_text, file)
}

// writing

/// Validates `text` as a config, then writes it and returns its loaded form. Nothing is
/// written when `text` does not load.
fn store<B: Backend>(
    gate: &Gate<B>,
    config_dir: &Path,
    home: &Home,
    text: &str,
) -> Result<ConfigFile, Error> {
    let path = config_path(config_dir);
    let file = parse(text, &path, gate, home)?;
    write_validated(gate, config_dir, &path, text, file)
}

/// Writes an already-validated config's text to `config.toml`, atomically from the
/// user's point of view: the text is written to a sibling file first, then that
/// sibling is renamed over `config.toml`. A write that fails partway (a full disk, a
/// crash) leaves the user's existing file exactly as it was, instead of a truncated or
/// empty one.
fn write_validated<B: Backend>(
    gate: &Gate<B>,
    config_dir: &Path,
    path: &Path,
    text: &str,
    file: ConfigFile,
) -> Result<ConfigFile, Error> {
    gate.create_own_dir_all(config_dir)?;
    let sibling = sibling_path(config_dir);
    gate.write_own_file(&sibling, text.as_bytes())?;
    gate.rename_own(&sibling, path)?;
    Ok(file)
}

fn sibling_path(config_dir: &Path) -> PathBuf {
    config_dir.join(format!("{FILE_NAME}.new"))
}

// parsing

fn parse<B: Backend>(
    text: &str,
    path: &Path,
    gate: &Gate<B>,
    home: &Home,
) -> Result<ConfigFile, Error> {
    let document = parse_syntax(text, path)?;
    validate_shape(&document, path)?;

    let raw: Config = toml_edit::de::from_str(text).map_err(|source| Error::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    let roots = raw
        .roots
        .iter()
        .map(|root| gate.check_root_entry(&expand_home(root, home)))
        .collect::<Result<Vec<_>, _>>()?;
    refuse_duplicate_roots(&raw.roots, &roots, path)?;

    let config = Config { roots, ..raw };
    Ok(ConfigFile { document, config })
}

/// Parses `text` as an editable document. The typed reading is a second, separate parse
/// of the same text (`toml_edit::de::from_str` in `parse`, not
/// `toml_edit::de::from_document` on this result): that path drops the source spans that
/// give unknown-key errors their line number.
fn parse_syntax(text: &str, path: &Path) -> Result<DocumentMut, Error> {
    text.parse::<DocumentMut>().map_err(|source| Error::Parse {
        path: path.to_path_buf(),
        source: source.into(),
    })
}

/// Refuses a `walk` that is not a table and a `roots` that is not an array, so the
/// `as_table_like`/`as_array` invariants `document.rs` relies on hold by construction —
/// serde's derived `Deserialize` for `Walk` would otherwise also accept a positional
/// sequence such as `walk = [true, false, false]` with no error.
fn validate_shape(document: &DocumentMut, path: &Path) -> Result<(), Error> {
    if let Some(item) = document.get("walk")
        && item.as_table_like().is_none()
    {
        return Err(Error::InvalidShape {
            path: path.to_path_buf(),
            key: "walk",
            expected: "a table",
        });
    }
    if let Some(item) = document.get("roots")
        && item.as_array().is_none()
    {
        return Err(Error::InvalidShape {
            path: path.to_path_buf(),
            key: "roots",
            expected: "an array",
        });
    }
    Ok(())
}

/// Refuses two `roots` entries that read as the same path once `~` is expanded and dots
/// are resolved, naming both as the user spelled them. A symlinked entry never reaches
/// this check: `check_root_entry` already refuses it.
fn refuse_duplicate_roots(
    written: &[PathBuf],
    checked: &[PathBuf],
    path: &Path,
) -> Result<(), Error> {
    for (i, resolved) in checked.iter().enumerate() {
        if let Some(j) = checked[..i].iter().position(|other| other == resolved) {
            return Err(Error::DuplicateRoot {
                path: path.to_path_buf(),
                a: written[j].clone(),
                b: written[i].clone(),
                resolved: resolved.clone(),
            });
        }
    }
    Ok(())
}

fn load_error(source: fs::Error) -> Error {
    match &source {
        fs::Error::Io { source: io, .. } if io.kind() == io::ErrorKind::NotFound => Error::NotFound,
        _ => Error::Fs(source),
    }
}

/// The user's `config.toml` inside rosie's config folder.
pub fn config_path(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE_NAME)
}

// home paths

/// Expands a leading `~` component against the injected home. A path not starting with
/// `~` is returned unchanged.
fn expand_home(path: &Path, home: &Home) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use crate::fs::Bounds;
    use crate::fs::fake::FakeBackend;

    use super::*;

    const HOME: &str = "/Users/me";
    const CONFIG_DIR: &str = "/Users/me/.config/rosie";
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

    #[test]
    fn loads_a_config_with_roots_and_walk_flags() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/code");
        fake.add_file(
            "/Users/me/.config/rosie/config.toml",
            "roots = [\"/Users/me/code\"]\n[walk]\nenter_bundles = true\n",
        );
        let gate = gate(&fake);

        let file = load(&gate, config_dir(), &home()).expect("load config");

        assert_eq!(file.config().roots, vec![PathBuf::from("/Users/me/code")]);
        assert!(file.config().walk.enter_bundles);
        assert!(!file.config().walk.enter_mounts);
    }

    #[test]
    fn reports_not_found_when_no_config_exists() {
        let fake = FakeBackend::new();
        let gate = gate(&fake);

        let result = load(&gate, config_dir(), &home());

        assert!(matches!(result, Err(Error::NotFound)));
    }

    #[test]
    fn an_unknown_key_is_a_load_error_naming_the_key_and_its_line() {
        let fake = FakeBackend::new();
        fake.add_file(
            "/Users/me/.config/rosie/config.toml",
            "roots = []\n[walk]\nenter_bundle = true\n",
        );
        let gate = gate(&fake);

        let result = load(&gate, config_dir(), &home());

        let Err(error @ Error::Parse { .. }) = result else {
            panic!("expected a parse error, got {result:?}");
        };
        let message = error.to_string();
        assert!(message.contains("enter_bundle"), "{message}");
        assert!(message.contains("line 3"), "{message}");
    }

    #[test]
    fn expands_a_leading_tilde_against_home() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/Applications");
        fake.add_file(
            "/Users/me/.config/rosie/config.toml",
            "roots = [\"~/Applications\"]\n",
        );
        let gate = gate(&fake);

        let file = load(&gate, config_dir(), &home()).expect("load config");

        assert_eq!(
            file.config().roots,
            vec![PathBuf::from("/Users/me/Applications")]
        );
    }

    #[test]
    fn a_root_that_does_not_exist_on_disk_is_valid() {
        let fake = FakeBackend::new();
        fake.add_file(
            "/Users/me/.config/rosie/config.toml",
            "roots = [\"/opt/homebrew/Cellar\"]\n",
        );
        let gate = gate(&fake);

        let file = load(&gate, config_dir(), &home()).expect("missing root is fine");

        assert_eq!(
            file.config().roots,
            vec![PathBuf::from("/opt/homebrew/Cellar")]
        );
    }

    #[test]
    fn a_root_through_an_existing_symlink_component_is_a_load_error_naming_it() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/elsewhere");
        fake.add_symlink("/Users/me/code", "elsewhere");
        fake.add_file(
            "/Users/me/.config/rosie/config.toml",
            "roots = [\"/Users/me/code/app\"]\n",
        );
        let gate = gate(&fake);

        let result = load(&gate, config_dir(), &home());

        let Err(Error::Fs(fs::Error::Symlink { link, .. })) = result else {
            panic!("expected a symlink load error, got {result:?}");
        };
        assert_eq!(link, PathBuf::from("/Users/me/code"));
    }

    #[test]
    fn seeds_a_fresh_config_from_the_default_text() {
        let fake = FakeBackend::new();
        fake.add_dir(HOME);
        let gate = gate(&fake);
        let default_text = "roots = []\n[walk]\nenter_bundles = false\n";

        let seeded = seed(&gate, config_dir(), &home(), default_text).expect("seed config");

        assert_eq!(seeded.config(), &Config::default());
        assert_eq!(
            gate.read_own_file(&config_path(config_dir()))
                .expect("config written"),
            default_text.as_bytes()
        );
    }

    #[test]
    fn refuses_to_seed_default_text_that_does_not_parse() {
        let fake = FakeBackend::new();
        let gate = gate(&fake);

        let result = seed(&gate, config_dir(), &home(), "not valid toml [[[");

        let Err(error @ Error::InvalidDefault(_)) = result else {
            panic!("expected an invalid default, got {result:?}");
        };
        let message = error.to_string();
        assert!(message.contains(DEFAULT_LABEL), "{message}");
        assert!(!message.contains("/Users/me/.config"), "{message}");
        assert!(!fake.exists("/Users/me/.config/rosie/config.toml"));
    }

    #[test]
    fn edits_of_the_shipped_default_keep_its_comments_and_tilde_entries() {
        let default_text = include_str!("../../config.toml");
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/code");
        let gate = gate(&fake);
        let comment_lines = |text: &str| text.lines().filter(|l| l.contains('#')).count();
        let tilde_entries = |text: &str| text.matches("\"~/").count();

        let seeded = seed(&gate, config_dir(), &home(), default_text).expect("seed");
        let added = roots::add(&gate, config_dir(), &home(), &seeded, Path::new("~/code"))
            .expect("add a root");
        let set = keys::set(
            &gate,
            config_dir(),
            &home(),
            &added,
            WalkFlag::EnterBundles,
            "true",
        )
        .expect("set a flag");
        let unset = keys::unset(&gate, config_dir(), &home(), &set, WalkFlag::EnterBundles)
            .expect("unset the flag");
        roots::remove(
            &gate,
            config_dir(),
            &home(),
            &unset,
            Path::new("~/.cargo/registry"),
        )
        .expect("remove the root under the tool-cache comment");

        let text = String::from_utf8(
            gate.read_own_file(&config_path(config_dir()))
                .expect("config written"),
        )
        .expect("UTF-8");
        assert_eq!(comment_lines(&text), comment_lines(default_text), "{text}");
        assert_eq!(tilde_entries(&text), tilde_entries(default_text), "{text}");
        assert!(text.contains("\"~/code\""), "{text}");
        assert!(!text.contains("~/.cargo/registry"), "{text}");
        assert!(!text.contains("enter_bundles"), "{text}");
    }

    #[test]
    fn a_non_table_walk_is_a_load_error_naming_the_key_instead_of_panicking() {
        let fake = FakeBackend::new();
        fake.add_file(
            "/Users/me/.config/rosie/config.toml",
            "roots = []\nwalk = [true, false, false]\n",
        );
        let gate = gate(&fake);

        let result = load(&gate, config_dir(), &home());

        assert!(
            matches!(result, Err(Error::InvalidShape { key: "walk", .. })),
            "{result:?}"
        );
    }

    #[test]
    fn a_non_array_roots_is_a_load_error_naming_the_key() {
        let fake = FakeBackend::new();
        fake.add_file("/Users/me/.config/rosie/config.toml", "roots = \"nope\"\n");
        let gate = gate(&fake);

        let result = load(&gate, config_dir(), &home());

        assert!(
            matches!(result, Err(Error::InvalidShape { key: "roots", .. })),
            "{result:?}"
        );
    }

    #[test]
    fn two_spellings_of_the_same_root_are_a_load_error_naming_both() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/code");
        fake.add_file(
            "/Users/me/.config/rosie/config.toml",
            "roots = [\"~/code\", \"/Users/me/code\"]\n",
        );
        let gate = gate(&fake);

        let result = load(&gate, config_dir(), &home());

        let Err(error @ Error::DuplicateRoot { .. }) = result else {
            panic!("expected a duplicate-root error, got {result:?}");
        };
        let message = error.to_string();
        assert!(message.contains("~/code"), "{message}");
        assert!(message.contains("/Users/me/code"), "{message}");
    }

    #[test]
    fn load_or_seed_seeds_only_when_no_config_exists() {
        let fake = FakeBackend::new();
        fake.add_dir(HOME);
        let gate = gate(&fake);

        let first = load_or_seed(&gate, config_dir(), &home(), "roots = []\n").expect("seed once");
        gate.write_own_file(&config_path(config_dir()), b"roots = [\"/Users/me\"]\n")
            .expect("simulate a user edit");
        let second = load_or_seed(&gate, config_dir(), &home(), "roots = []\n")
            .expect("load the edited file, not the default again");

        assert_eq!(first.config(), &Config::default());
        assert_eq!(second.config().roots, vec![PathBuf::from("/Users/me")]);
    }

    #[test]
    fn a_failed_write_leaves_the_original_config_untouched() {
        use std::io::ErrorKind;

        use crate::fs::Op;

        let fake = FakeBackend::new();
        fake.add_dir(HOME);
        let original = "roots = []\n[walk]\nenter_bundles = true\n";
        fake.add_file("/Users/me/.config/rosie/config.toml", original);
        let gate = gate(&fake);
        fake.fail_on(
            sibling_path(config_dir()),
            Op::WriteFile,
            ErrorKind::StorageFull,
        );

        let result = store(
            &gate,
            config_dir(),
            &home(),
            "roots = []\n[walk]\nenter_bundles = false\n",
        );

        assert!(matches!(result, Err(Error::Fs(_))), "{result:?}");
        assert_eq!(
            gate.read_own_file(&config_path(config_dir()))
                .expect("original config still there"),
            original.as_bytes()
        );
        assert!(!fake.exists(sibling_path(config_dir())));
    }

    #[test]
    fn a_default_with_an_invalid_shape_is_reported_as_an_invalid_default_and_nothing_is_written() {
        let fake = FakeBackend::new();
        fake.add_dir(HOME);
        let gate = gate(&fake);

        let result = seed(&gate, config_dir(), &home(), "walk = [true]\n");

        assert!(
            matches!(result, Err(Error::InvalidDefault(_))),
            "{result:?}"
        );
        assert!(!fake.exists(config_path(config_dir())));
    }

    #[test]
    fn a_default_with_duplicate_roots_is_reported_as_an_invalid_default_and_nothing_is_written() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/code");
        let gate = gate(&fake);

        let result = seed(
            &gate,
            config_dir(),
            &home(),
            "roots = [\"~/code\", \"/Users/me/code\"]\n",
        );

        assert!(
            matches!(result, Err(Error::InvalidDefault(_))),
            "{result:?}"
        );
        assert!(!fake.exists(config_path(config_dir())));
    }

    #[test]
    fn a_successful_write_leaves_no_sibling_file_behind() {
        let fake = FakeBackend::new();
        fake.add_dir(HOME);
        let gate = gate(&fake);

        seed(&gate, config_dir(), &home(), "roots = []\n").expect("seed config");

        assert!(!fake.exists(sibling_path(config_dir())));
    }
}
