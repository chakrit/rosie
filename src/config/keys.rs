//! `config get` / `set` / `unset` and the key registry pickers use
//! (`docs/spec/cli.md#management-commands`, `#pickers`).

use std::path::Path;

use super::{Config, ConfigFile, Error, document, store};
use crate::fs::{Backend, Gate, Home};

/// A config key, as `rosie config` and `config get` show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Changed only through `rosie roots add` / `remove`.
    Roots,
    Walk(WalkFlag),
}

/// A `[walk]` flag: the keys `config set` and `config unset` change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkFlag {
    EnterBundles,
    EnterMounts,
    EnterPlaceholders,
}

impl Key {
    /// Every key, for `rosie config` and the `config get` picker.
    pub const ALL: [Key; 4] = [
        Key::Roots,
        Key::Walk(WalkFlag::EnterBundles),
        Key::Walk(WalkFlag::EnterMounts),
        Key::Walk(WalkFlag::EnterPlaceholders),
    ];

    pub fn name(self) -> &'static str {
        match self {
            Key::Roots => "roots",
            Key::Walk(flag) => flag.name(),
        }
    }

    pub fn parse(name: &str) -> Result<Key, Error> {
        Key::ALL
            .into_iter()
            .find(|key| key.name() == name)
            .ok_or_else(|| Error::UnknownKey {
                name: name.to_string(),
            })
    }
}

impl WalkFlag {
    /// Every settable key, for the `config set` and `config unset` pickers.
    pub const ALL: [WalkFlag; 3] = [
        WalkFlag::EnterBundles,
        WalkFlag::EnterMounts,
        WalkFlag::EnterPlaceholders,
    ];

    /// Values `config set` accepts, for the `set` picker (`docs/spec/cli.md#pickers`).
    pub const ALLOWED_VALUES: [&'static str; 2] = ["true", "false"];

    pub fn name(self) -> &'static str {
        match self {
            WalkFlag::EnterBundles => "walk.enter_bundles",
            WalkFlag::EnterMounts => "walk.enter_mounts",
            WalkFlag::EnterPlaceholders => "walk.enter_placeholders",
        }
    }

    /// The key's name inside the `[walk]` table.
    pub(super) fn field(self) -> &'static str {
        match self {
            WalkFlag::EnterBundles => "enter_bundles",
            WalkFlag::EnterMounts => "enter_mounts",
            WalkFlag::EnterPlaceholders => "enter_placeholders",
        }
    }

    /// Parses a key `config set` / `unset` names. `roots` is refused, pointing at
    /// `rosie roots`.
    pub fn parse(name: &str) -> Result<WalkFlag, Error> {
        match Key::parse(name)? {
            Key::Walk(flag) => Ok(flag),
            Key::Roots => Err(Error::RootsNotSettable),
        }
    }
}

/// The current value of `key`, for `config get` and `config` (show every key). Roots
/// print one per line.
pub fn get(key: Key, config: &Config) -> String {
    match key {
        Key::Roots => config
            .roots
            .iter()
            .map(|root| root.display().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        Key::Walk(flag) => config.walk.flag(flag).to_string(),
    }
}

/// `config set <key> <value>`: writes the flag into `config.toml`.
pub fn set<B: Backend>(
    gate: &Gate<B>,
    config_dir: &Path,
    home: &Home,
    file: &ConfigFile,
    flag: WalkFlag,
    value: &str,
) -> Result<ConfigFile, Error> {
    let on = parse_bool(flag, value)?;

    let edited = document::with_flag(&file.document, flag, on);
    store(gate, config_dir, home, &edited.to_string())
}

/// `config unset <key>`: deletes the flag from `config.toml` so its default applies. A
/// flag the file does not set is left alone and nothing is written.
pub fn unset<B: Backend>(
    gate: &Gate<B>,
    config_dir: &Path,
    home: &Home,
    file: &ConfigFile,
    flag: WalkFlag,
) -> Result<ConfigFile, Error> {
    let Some(edited) = document::without_flag(&file.document, flag) else {
        return Ok(file.clone());
    };
    store(gate, config_dir, home, &edited.to_string())
}

fn parse_bool(flag: WalkFlag, value: &str) -> Result<bool, Error> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(Error::InvalidValue {
            key: flag.name(),
            value: value.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::fs::Bounds;
    use crate::fs::fake::FakeBackend;

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

    fn file_text(gate: &Gate<&FakeBackend>) -> String {
        let bytes = gate
            .read_own_file(Path::new(CONFIG_FILE))
            .expect("config file exists");
        String::from_utf8(bytes).expect("config file is UTF-8")
    }

    #[test]
    fn parses_a_key_by_its_dotted_name() {
        assert_eq!(
            Key::parse("walk.enter_bundles").expect("known key"),
            Key::Walk(WalkFlag::EnterBundles)
        );
        assert!(matches!(
            Key::parse("nonsense"),
            Err(Error::UnknownKey { name }) if name == "nonsense"
        ));
    }

    #[test]
    fn roots_is_refused_as_a_settable_key() {
        assert!(matches!(
            WalkFlag::parse("roots"),
            Err(Error::RootsNotSettable)
        ));
    }

    #[test]
    fn set_writes_the_flag_into_the_file() {
        let fake = FakeBackend::new();
        fake.add_file(CONFIG_FILE, "roots = []\n");
        let gate = gate(&fake);
        let file = load(&gate, config_dir(), &home()).expect("load");

        let updated = set(
            &gate,
            config_dir(),
            &home(),
            &file,
            WalkFlag::EnterMounts,
            "true",
        )
        .expect("set a flag");

        assert_eq!(
            get(Key::Walk(WalkFlag::EnterMounts), updated.config()),
            "true"
        );
        let reloaded = load(&gate, config_dir(), &home()).expect("reload");
        assert!(reloaded.config().walk.enter_mounts);
    }

    #[test]
    fn refuses_a_value_that_is_not_true_or_false() {
        let fake = FakeBackend::new();
        fake.add_file(CONFIG_FILE, "roots = []\n");
        let gate = gate(&fake);
        let file = load(&gate, config_dir(), &home()).expect("load");

        let result = set(
            &gate,
            config_dir(),
            &home(),
            &file,
            WalkFlag::EnterBundles,
            "yes",
        );

        assert!(matches!(result, Err(Error::InvalidValue { .. })));
        assert_eq!(file_text(&gate), "roots = []\n");
    }

    #[test]
    fn unset_deletes_the_key_from_the_file_and_the_default_applies() {
        let fake = FakeBackend::new();
        fake.add_file(
            CONFIG_FILE,
            "roots = []\n[walk]\nenter_bundles = true\nenter_mounts = true\n",
        );
        let gate = gate(&fake);
        let file = load(&gate, config_dir(), &home()).expect("load");

        unset(&gate, config_dir(), &home(), &file, WalkFlag::EnterBundles).expect("unset");

        let text = file_text(&gate);
        assert!(!text.contains("enter_bundles"), "{text}");
        assert!(text.contains("enter_mounts = true"), "{text}");
        let reloaded = load(&gate, config_dir(), &home()).expect("reload");
        assert!(!reloaded.config().walk.enter_bundles);
    }

    #[test]
    fn set_and_unset_keep_comments_and_the_other_keys_as_written() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/Applications");
        let original = "# my config\nroots = [\n  # apps\n  \"~/Applications\",\n]\n";
        fake.add_file(CONFIG_FILE, original);
        let gate = gate(&fake);
        let file = load(&gate, config_dir(), &home()).expect("load");

        let set_file = set(
            &gate,
            config_dir(),
            &home(),
            &file,
            WalkFlag::EnterBundles,
            "true",
        )
        .expect("set");
        let after_set = file_text(&gate);
        unset(
            &gate,
            config_dir(),
            &home(),
            &set_file,
            WalkFlag::EnterBundles,
        )
        .expect("unset");
        let after_unset = file_text(&gate);

        assert!(after_set.starts_with(original), "{after_set}");
        assert!(after_set.contains("enter_bundles = true"), "{after_set}");
        assert!(after_unset.starts_with(original), "{after_unset}");
        assert!(!after_unset.contains("enter_bundles"), "{after_unset}");
    }
}
