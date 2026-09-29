//! `rules`, `roots`, and `config` (`docs/spec/cli.md#management-commands`).

use std::path::{Path, PathBuf};

use super::args::{ConfigAction, RootsAction, RulesAction};
use super::console::Console;
use super::session::{PickKind, Session};
use super::{Error, ExitStatus};
use crate::config::{ConfigFile, Key, WalkFlag, keys, roots};
use crate::fs::Backend;
use crate::packs::{self, Network, RemovePack, Source};
use crate::rules::{Rule, Source as RuleSource};

impl<'a, B: Backend + Sync, N: Network, C: Console> Session<'a, B, N, C> {
    pub fn rules_command(&mut self, action: Option<RulesAction>) -> Result<ExitStatus, Error> {
        match action {
            None => self.show_rules(),
            Some(RulesAction::Pull { source }) => self.pull(source.as_deref()),
            Some(RulesAction::Remove { pack }) => self.remove_pack(pack),
        }
    }

    pub fn roots_command(&mut self, action: Option<RootsAction>) -> Result<ExitStatus, Error> {
        let file = self.config()?;
        match action {
            None => self.show_roots(&file),
            Some(RootsAction::Add { path }) => self.add_root(&file, &path),
            Some(RootsAction::Remove { path }) => self.remove_root(&file, path),
        }
    }

    pub fn config_command(&mut self, action: Option<ConfigAction>) -> Result<ExitStatus, Error> {
        let file = self.config()?;
        match action {
            None => self.show_config(&file),
            Some(ConfigAction::Get { key }) => self.get_key(&file, key),
            Some(ConfigAction::Set { key, value }) => self.set_key(&file, key, value),
            Some(ConfigAction::Unset { key }) => self.unset_key(&file, key),
        }
    }

    // rules

    fn show_rules(&mut self) -> Result<ExitStatus, Error> {
        let rules = self.rules(&[])?;
        for rule in rules.iter() {
            self.say(&rule_line(rule))?;
        }
        Ok(ExitStatus::Success)
    }

    fn pull(&mut self, source: Option<&str>) -> Result<ExitStatus, Error> {
        let source = match source {
            Some(text) => text.parse::<Source>()?,
            None => Source::default(),
        };

        let pulled = packs::pull(&self.store(), self.network, &source, self.env.now)?;
        self.say(&format!("pulled {}", pulled.source))?;
        for conflict in &pulled.conflicts {
            self.note(&format!("warning: {conflict}"))?;
        }
        Ok(ExitStatus::Success)
    }

    fn remove_pack(&mut self, pack: Option<String>) -> Result<ExitStatus, Error> {
        let pack = match pack {
            Some(pack) => pack,
            None => {
                let picker = self.picker("rosie rules remove <pack>")?;
                let packs = self
                    .store()
                    .sources()?
                    .into_iter()
                    .map(|source| {
                        let name = source.pack().to_string();
                        (format!("{name} ({source})"), name)
                    })
                    .collect();
                self.pick(picker, PickKind::Pack, packs)?
            }
        };

        let removed = RemovePack { pack }.run(&self.store())?;
        self.say(&format!("removed {removed}"))?;
        Ok(ExitStatus::Success)
    }

    // roots

    fn show_roots(&mut self, file: &ConfigFile) -> Result<ExitStatus, Error> {
        for root in roots::list(file.config()) {
            self.say(&root.display().to_string())?;
        }
        Ok(ExitStatus::Success)
    }

    fn add_root(&mut self, file: &ConfigFile, path: &Path) -> Result<ExitStatus, Error> {
        let path = self.typed_root(path);
        let dir = self.env.home.config_dir();

        roots::add(&self.own, &dir, &self.env.home, file, &path)?;
        self.say(&format!("added {}", path.display()))?;
        Ok(ExitStatus::Success)
    }

    fn remove_root(
        &mut self,
        file: &ConfigFile,
        path: Option<PathBuf>,
    ) -> Result<ExitStatus, Error> {
        let path = match path {
            Some(path) => self.typed_root(&path),
            None => {
                let picker = self.picker("rosie roots remove <path>")?;
                let options = roots::list(file.config())
                    .iter()
                    .map(|root| (root.display().to_string(), root.clone()))
                    .collect();
                self.pick(picker, PickKind::Root, options)?
            }
        };
        let dir = self.env.home.config_dir();

        roots::remove(&self.own, &dir, &self.env.home, file, &path)?;
        self.say(&format!("removed {}", path.display()))?;
        Ok(ExitStatus::Success)
    }

    /// A root as typed: a `~` path stays as it is for the config to expand, and a
    /// relative path is taken against the current folder.
    fn typed_root(&self, path: &Path) -> PathBuf {
        match path.starts_with("~") {
            true => path.to_path_buf(),
            false => self.absolute(path),
        }
    }

    // config

    fn show_config(&mut self, file: &ConfigFile) -> Result<ExitStatus, Error> {
        for key in Key::ALL {
            let value = keys::get(key, file.config());
            let line = match key {
                Key::Roots => format!("roots =\n{}", indented(&value)),
                Key::Walk(_) => format!("{} = {value}", key.name()),
            };
            self.say(&line)?;
        }
        Ok(ExitStatus::Success)
    }

    fn get_key(&mut self, file: &ConfigFile, key: Option<String>) -> Result<ExitStatus, Error> {
        let key = match key {
            Some(name) => Key::parse(&name)?,
            None => {
                let picker = self.picker("rosie config get <key>")?;
                let options = Key::ALL
                    .into_iter()
                    .map(|key| (key.name().to_owned(), key))
                    .collect();
                self.pick(picker, PickKind::Key, options)?
            }
        };

        self.say(&keys::get(key, file.config()))?;
        Ok(ExitStatus::Success)
    }

    fn set_key(
        &mut self,
        file: &ConfigFile,
        key: Option<String>,
        value: Option<String>,
    ) -> Result<ExitStatus, Error> {
        let usage = "rosie config set <key> <value>";
        let flag = self.walk_flag(key, usage)?;
        let value = match value {
            Some(value) => value,
            None => {
                let picker = self.picker(usage)?;
                let options = WalkFlag::ALLOWED_VALUES
                    .into_iter()
                    .map(|value| (value.to_owned(), value.to_owned()))
                    .collect();
                self.pick(picker, PickKind::Value, options)?
            }
        };
        let dir = self.env.home.config_dir();

        keys::set(&self.own, &dir, &self.env.home, file, flag, &value)?;
        self.say(&format!("{} = {value}", flag.name()))?;
        Ok(ExitStatus::Success)
    }

    fn unset_key(&mut self, file: &ConfigFile, key: Option<String>) -> Result<ExitStatus, Error> {
        let flag = self.walk_flag(key, "rosie config unset <key>")?;
        let dir = self.env.home.config_dir();

        let unset = keys::unset(&self.own, &dir, &self.env.home, file, flag)?;
        let default = keys::get(Key::Walk(flag), unset.config());
        self.say(&format!("{} = {default} (default)", flag.name()))?;
        Ok(ExitStatus::Success)
    }

    fn walk_flag(&mut self, key: Option<String>, usage: &str) -> Result<WalkFlag, Error> {
        match key {
            Some(name) => Ok(WalkFlag::parse(&name)?),
            None => {
                let picker = self.picker(usage)?;
                let options = WalkFlag::ALL
                    .into_iter()
                    .map(|flag| (flag.name().to_owned(), flag))
                    .collect();
                self.pick(picker, PickKind::Key, options)
            }
        }
    }
}

/// `node-modules (pack rosie)`: the name `--only` takes, and its pack.
fn rule_line(rule: &Rule) -> String {
    let id = rule.id();
    match rule.source() {
        RuleSource::Pack => format!("{} (pack {})", id.rule, id.pack),
        RuleSource::UserOverride => {
            format!("{} (pack {}, replaced by your rules)", id.rule, id.pack)
        }
    }
}

fn indented(lines: &str) -> String {
    lines
        .lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
