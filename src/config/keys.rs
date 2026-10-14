//! `config get` / `set` / `unset` and the key registry pickers use
//! (`docs/spec/cli.md#management-commands`, `#pickers`).

use std::path::Path;

use toml_edit::Value;

use super::{ConfigFile, Error, document, store};
use crate::fs::{Backend, EnterLinks, FsContext};

/// A config key, as `rosie config` and `config get` show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Changed only through `rosie roots add` / `remove`.
    Roots,
    Walk(WalkKey),
}

/// A `[walk]` key: the keys `config set` and `config unset` change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkKey {
    EnterBundles,
    EnterMounts,
    EnterPlaceholders,
    EnterLinks,
}

/// A `[walk]` setting that opens a kind of closed folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkSetting {
    /// `enter_placeholders`: a dataless cloud placeholder.
    Placeholders,
    /// `enter_mounts`: a mount root on another volume.
    Mounts,
    /// `enter_bundles`: a bundle folder.
    Bundles,
}

impl Key {
    /// Every key, for `rosie config` and the `config get` picker.
    pub const ALL: [Key; 5] = [
        Key::Roots,
        Key::Walk(WalkKey::EnterBundles),
        Key::Walk(WalkKey::EnterMounts),
        Key::Walk(WalkKey::EnterPlaceholders),
        Key::Walk(WalkKey::EnterLinks),
    ];

    pub fn name(self) -> &'static str {
        match self {
            Key::Roots => "roots",
            Key::Walk(key) => key.name(),
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

impl WalkKey {
    /// Every settable key, for the `config set` and `config unset` pickers.
    pub const ALL: [WalkKey; 4] = [
        WalkKey::EnterBundles,
        WalkKey::EnterMounts,
        WalkKey::EnterPlaceholders,
        WalkKey::EnterLinks,
    ];

    pub fn name(self) -> &'static str {
        match self {
            WalkKey::EnterBundles => "walk.enter_bundles",
            WalkKey::EnterMounts => "walk.enter_mounts",
            WalkKey::EnterPlaceholders => "walk.enter_placeholders",
            WalkKey::EnterLinks => "walk.enter_links",
        }
    }

    /// Values `config set` accepts, for the `set` picker (`docs/spec/cli.md#pickers`).
    pub fn allowed_values(self) -> Vec<&'static str> {
        match self {
            WalkKey::EnterBundles | WalkKey::EnterMounts | WalkKey::EnterPlaceholders => {
                vec!["true", "false"]
            }
            WalkKey::EnterLinks => EnterLinks::ALL.into_iter().map(EnterLinks::label).collect(),
        }
    }

    /// The key's name inside the `[walk]` table.
    pub(super) fn field(self) -> &'static str {
        match self {
            WalkKey::EnterBundles => "enter_bundles",
            WalkKey::EnterMounts => "enter_mounts",
            WalkKey::EnterPlaceholders => "enter_placeholders",
            WalkKey::EnterLinks => "enter_links",
        }
    }

    /// Parses a key `config set` / `unset` names. `roots` is refused, pointing at
    /// `rosie roots`.
    pub fn parse(name: &str) -> Result<WalkKey, Error> {
        match Key::parse(name)? {
            Key::Walk(key) => Ok(key),
            Key::Roots => Err(Error::RootsNotSettable),
        }
    }

    /// The TOML value `config set` writes for `value`, which must be one of
    /// [`WalkKey::allowed_values`].
    fn value(self, value: &str) -> Result<Value, Error> {
        let invalid = || Error::InvalidValue {
            key: self.name(),
            value: value.to_string(),
            allowed: self.allowed_values().join(", "),
        };
        match self {
            WalkKey::EnterBundles | WalkKey::EnterMounts | WalkKey::EnterPlaceholders => {
                match value {
                    "true" => Ok(Value::from(true)),
                    "false" => Ok(Value::from(false)),
                    _ => Err(invalid()),
                }
            }
            WalkKey::EnterLinks => EnterLinks::parse(value)
                .map(|level| Value::from(level.label()))
                .ok_or_else(invalid),
        }
    }
}

/// The current value of `key`, for `config get` and `config` (show every key). Roots
/// print one per line, as written.
pub fn get(key: Key, file: &ConfigFile) -> String {
    match key {
        Key::Roots => file
            .roots()
            .iter()
            .map(|root| root.display().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        Key::Walk(key) => file.walk().value(key),
    }
}

/// `config set <key> <value>`: writes the key into `config.toml`.
pub fn set<B: Backend>(
    ctx: &FsContext<B>,
    config_dir: &Path,
    file: &ConfigFile,
    key: WalkKey,
    value: &str,
) -> Result<ConfigFile, Error> {
    let value = key.value(value)?;

    let edited = document::with_walk_value(&file.document, key, value);
    store(ctx, config_dir, &edited.to_string())
}

/// `config unset <key>`: deletes the key from `config.toml` so its default applies. A
/// key the file does not set is left alone and nothing is written.
pub fn unset<B: Backend>(
    ctx: &FsContext<B>,
    config_dir: &Path,
    file: &ConfigFile,
    key: WalkKey,
) -> Result<ConfigFile, Error> {
    let Some(edited) = document::without_walk_key(&file.document, key) else {
        return Ok(file.clone());
    };
    store(ctx, config_dir, &edited.to_string())
}

#[cfg(test)]
mod tests {
    use crate::fs::fake::FakeBackend;
    use crate::fs::{Bounds, FsOp, Paths, Rosie};

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

    fn file_text(ctx: &FsContext<&FakeBackend>) -> String {
        let bytes = Rosie::read(Path::new(CONFIG_FILE))
            .perform(ctx)
            .expect("config file exists");
        String::from_utf8(bytes).expect("config file is UTF-8")
    }

    #[test]
    fn parses_a_key_by_its_dotted_name() {
        assert_eq!(
            Key::parse("walk.enter_bundles").expect("known key"),
            Key::Walk(WalkKey::EnterBundles)
        );
        assert!(matches!(
            Key::parse("nonsense"),
            Err(Error::UnknownKey { name }) if name == "nonsense"
        ));
    }

    #[test]
    fn roots_is_refused_as_a_settable_key() {
        assert!(matches!(
            WalkKey::parse("roots"),
            Err(Error::RootsNotSettable)
        ));
    }

    #[test]
    fn set_writes_the_flag_into_the_file() {
        let fake = FakeBackend::new();
        fake.add_file(CONFIG_FILE, "roots = []\n");
        let ctx = ctx(&fake);
        let file = load(&ctx, config_dir()).expect("load");

        let updated =
            set(&ctx, config_dir(), &file, WalkKey::EnterMounts, "true").expect("set a flag");

        assert_eq!(get(Key::Walk(WalkKey::EnterMounts), &updated), "true");
        let reloaded = load(&ctx, config_dir()).expect("reload");
        assert!(reloaded.walk().enter_mounts);
    }

    #[test]
    fn refuses_a_value_that_is_not_true_or_false() {
        let fake = FakeBackend::new();
        fake.add_file(CONFIG_FILE, "roots = []\n");
        let ctx = ctx(&fake);
        let file = load(&ctx, config_dir()).expect("load");

        let result = set(&ctx, config_dir(), &file, WalkKey::EnterBundles, "yes");

        assert!(matches!(result, Err(Error::InvalidValue { .. })));
        assert_eq!(file_text(&ctx), "roots = []\n");
    }

    #[test]
    fn sets_enter_links_to_a_named_level() {
        let fake = FakeBackend::new();
        fake.add_file(CONFIG_FILE, "roots = []\n");
        let ctx = ctx(&fake);
        let file = load(&ctx, config_dir()).expect("load");
        assert_eq!(get(Key::Walk(WalkKey::EnterLinks), &file), "none");

        let updated =
            set(&ctx, config_dir(), &file, WalkKey::EnterLinks, "roots").expect("set a level");

        assert_eq!(get(Key::Walk(WalkKey::EnterLinks), &updated), "roots");
        assert!(file_text(&ctx).contains("enter_links = \"roots\""));
        let reloaded = load(&ctx, config_dir()).expect("reload");
        assert_eq!(reloaded.walk().enter_links, EnterLinks::Roots);
    }

    #[test]
    fn refuses_an_enter_links_value_that_is_not_a_level() {
        let fake = FakeBackend::new();
        fake.add_file(CONFIG_FILE, "roots = []\n");
        let ctx = ctx(&fake);
        let file = load(&ctx, config_dir()).expect("load");

        for value in ["true", "1", "everything"] {
            let result = set(&ctx, config_dir(), &file, WalkKey::EnterLinks, value);

            assert!(
                matches!(&result, Err(Error::InvalidValue { allowed, .. }) if allowed == "none, roots, all"),
                "{value}: {result:?}"
            );
        }
        assert_eq!(file_text(&ctx), "roots = []\n");
    }

    #[test]
    fn unset_deletes_the_key_from_the_file_and_the_default_applies() {
        let fake = FakeBackend::new();
        fake.add_file(
            CONFIG_FILE,
            "roots = []\n[walk]\nenter_bundles = true\nenter_mounts = true\n",
        );
        let ctx = ctx(&fake);
        let file = load(&ctx, config_dir()).expect("load");

        unset(&ctx, config_dir(), &file, WalkKey::EnterBundles).expect("unset");

        let text = file_text(&ctx);
        assert!(!text.contains("enter_bundles"), "{text}");
        assert!(text.contains("enter_mounts = true"), "{text}");
        let reloaded = load(&ctx, config_dir()).expect("reload");
        assert!(!reloaded.walk().enter_bundles);
    }

    #[test]
    fn set_and_unset_keep_comments_and_the_other_keys_as_written() {
        let fake = FakeBackend::new();
        fake.add_dir("/Users/me/Applications");
        let original = "# my config\nroots = [\n  # apps\n  \"~/Applications\",\n]\n";
        fake.add_file(CONFIG_FILE, original);
        let ctx = ctx(&fake);
        let file = load(&ctx, config_dir()).expect("load");

        let set_file = set(&ctx, config_dir(), &file, WalkKey::EnterBundles, "true").expect("set");
        let after_set = file_text(&ctx);
        unset(&ctx, config_dir(), &set_file, WalkKey::EnterBundles).expect("unset");
        let after_unset = file_text(&ctx);

        assert!(after_set.starts_with(original), "{after_set}");
        assert!(after_set.contains("enter_bundles = true"), "{after_set}");
        assert!(after_unset.starts_with(original), "{after_unset}");
        assert!(!after_unset.contains("enter_bundles"), "{after_unset}");
    }
}
