//! One command's run: the `sudo rosie` refusal, then the FS contexts, config, and rules each
//! command needs, loaded through the modules that own them.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::args::{Place, RuleFlags, UserCommand, Verb};
use super::console::Console;
use super::root::refuse_root;
use super::shown::Shown;
use super::{Env, Error, ExitStatus};
use crate::config::{self, Config, ConfigFile};
use crate::fs::{Backend, FsContext, FsOp, ROOT_UID, UserInput};
use crate::output::{Line, Mark};
use crate::packs::{self, FirstRun, Network, SettlePacks, Store};
use crate::rules::{Name, RuleLayers, RuleSet};
use crate::scan::{Scanner, Settings};

/// Proof that stdin and stdout are terminals, which [`Session::pick`] takes: only
/// [`Session::picker`] makes one.
pub(super) struct Picker(());

/// What a picker lets the user choose, so the prompt and the empty-list message each
/// word it correctly (`docs/spec/cli.md#pickers`): the prompt names one item ("a
/// root"), the empty case names the missing collection ("roots").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    Root,
    Pack,
    App,
    Key,
    Value,
}

impl PickKind {
    fn prompt(self) -> &'static str {
        match self {
            PickKind::Root => "a root",
            PickKind::Pack => "a pack",
            PickKind::App => "an app",
            PickKind::Key => "a key",
            PickKind::Value => "a value",
        }
    }

    pub(super) fn empty(self) -> &'static str {
        match self {
            PickKind::Root => "roots",
            PickKind::Pack => "pulled packs",
            PickKind::App => "installed apps",
            PickKind::Key => "config keys",
            PickKind::Value => "values",
        }
    }
}

pub(super) struct Session<'a, B: Backend, N: Network, C: Console> {
    backend: &'a B,
    pub network: &'a N,
    pub console: &'a mut C,
    pub env: &'a Env,
    /// Acts for the user on rosie's own data; it has no roots, so it deletes no cleanup
    /// target.
    pub own: FsContext<&'a B>,
}

impl<'a, B: Backend + Sync, N: Network, C: Console> Session<'a, B, N, C> {
    /// Refuses `sudo rosie …` (`docs/spec/safety.md#refusing-sudo-rosie`) before anything
    /// reads rosie's data. `typed` are the arguments to repeat without sudo. The user
    /// rosie acts for is the owner of the home folder, as in the elevated child.
    pub fn open(
        backend: &'a B,
        network: &'a N,
        console: &'a mut C,
        env: &'a Env,
        typed: Vec<String>,
    ) -> Result<Self, Error> {
        let probe = FsContext::new(backend, env.paths.bounds(Vec::new(), ROOT_UID))?;
        refuse_root(&probe, env.pid, typed)?;
        let user_uid = UserInput::home_owner().perform(&probe)?;

        let own = FsContext::new(backend, env.paths.bounds(Vec::new(), user_uid))?;
        Ok(Session {
            backend,
            network,
            console,
            env,
            own,
        })
    }

    pub fn dispatch(&mut self, command: UserCommand) -> Result<ExitStatus, Error> {
        match command {
            UserCommand::Scan(args) => self.scan(args),
            UserCommand::Clean(args) => self.clean(args.target, Place::Mode(Verb::Clean)),
            UserCommand::Check(args) => self.check(args),
            UserCommand::Run { plan, conflicts } => self.run_plan(&plan, conflicts.conflicts),
            UserCommand::Rules { action } => self.rules_command(action),
            UserCommand::Roots { action } => self.roots_command(action),
            UserCommand::Config { action } => self.config_command(action),
        }
    }

    // what commands load

    /// The config the scans use: the user's config file with every root located on disk.
    pub fn config(&mut self) -> Result<Config, Error> {
        let file = self.config_file()?;
        Ok(file.locate(&self.own)?)
    }

    /// The user's config file, as `rosie roots` and `rosie config` edit it. A missing one
    /// is seeded (`docs/spec/safety.md#roots`) from the default source's `config.toml`,
    /// whose download also installs the default pack when none is installed yet.
    pub fn config_file(&mut self) -> Result<ConfigFile, Error> {
        let dir = self.env.paths.config_dir();
        match config::load(&self.own, &dir) {
            Err(config::Error::NotFound) => {}
            loaded => return Ok(loaded?),
        }

        let seed = packs::seed_config(&self.store(), self.network, self.env.now)?;
        self.show_first_run(&seed.first_run)?;
        let seeded = config::load_or_seed(&self.own, &dir, &seed.config)?;
        let path = config::config_path(&dir);
        self.note(
            Mark::Done,
            &format!("created {} from the default config", path.display()),
        )?;
        Ok(seeded)
    }

    /// The rules of every layer, narrowed to `only` when it names any. With no pack
    /// installed, the default source is pulled first; old packs draw the age warning
    /// (`docs/spec/rules.md#packs`).
    pub fn rules(&mut self, only: &[Name]) -> Result<RuleSet, Error> {
        let store = self.store();
        let first_run = packs::first_run(&store, self.network, self.env.now)?;
        let settled = SettlePacks.run(&store)?;
        let installed = settled.installed()?;
        let packs = settled.pack_folders()?;
        let warning = packs::age_warning(&installed, self.env.now);

        self.show_first_run(&first_run)?;
        if let Some(warning) = warning {
            self.note(Mark::Warning, &warning.to_string())?;
        }

        let layers = RuleLayers::new(packs, &self.env.paths.config_dir());
        let rules = RuleSet::load(&self.own, &layers)?;
        match only {
            [] => Ok(rules),
            names => Ok(rules.only(names)?),
        }
    }

    /// A gate bounded by the config's roots: the only kind that deletes cleanup targets.
    pub fn roots_ctx(&self, config: &Config) -> Result<FsContext<&'a B>, Error> {
        let bounds = self
            .env
            .paths
            .bounds(config.roots.clone(), self.own.user_uid());
        Ok(FsContext::new(self.backend, bounds)?)
    }

    /// The scanner of the rule-driven scans, `tree`, `caches` and `check`: the roots
    /// context, the rules `flags` keep, and `settings` with `flags` applied to its walk.
    pub fn rule_scanner(
        &mut self,
        config: &Config,
        settings: Settings,
        flags: &RuleFlags,
    ) -> Result<Scanner<&'a B>, Error> {
        let ctx = self.roots_ctx(config)?;
        let rules = self.rules(&flags.only)?;

        let settings = Settings {
            walk: flags.applied_to(settings.walk),
            ..settings
        };
        Ok(Scanner::new(ctx, rules, settings))
    }

    pub fn store(&self) -> Store<'_, &'a B> {
        Store::new(&self.own, &self.env.paths.data_dir())
    }

    // paths

    /// A path the user typed, taken against the current folder when relative.
    pub fn absolute(&self, path: &Path) -> PathBuf {
        match path.is_absolute() {
            true => path.to_path_buf(),
            false => self.env.cwd.join(path),
        }
    }

    /// How this session prints paths: under the injected home as `~`.
    pub fn tilde(&self) -> Shown<'a> {
        Shown {
            home: self.env.paths.home(),
        }
    }

    // output

    /// A line on stdout.
    pub fn say(&mut self, text: &str) -> io::Result<()> {
        writeln!(self.console.stdout(), "{text}")
    }

    /// A line on stderr, marked as `mark` (`docs/spec/cli.md#output`).
    pub fn note(&mut self, mark: Mark, text: &str) -> io::Result<()> {
        let line = self.console.stderr_palette().line(&Line::new(mark, text));
        writeln!(self.console.stderr(), "{line}")
    }

    fn show_first_run(&mut self, first_run: &FirstRun) -> io::Result<()> {
        let FirstRun::Pulled(pulled) = first_run else {
            return Ok(());
        };
        self.note(
            Mark::Note,
            &format!("no rule pack yet; pulled {}", pulled.source),
        )
    }

    // pickers

    /// Checks for a person at the terminal before a picker's options are even listed.
    /// Without a terminal on stdin and stdout, the command fails with a usage error
    /// (`docs/spec/cli.md#pickers`).
    pub fn picker(&self, usage: &str) -> Result<Picker, Error> {
        match self.console.terminals().interactive() {
            true => Ok(Picker(())),
            false => Err(Error::Usage(format!(
                "usage: {usage}; without a terminal rosie cannot show a picker"
            ))),
        }
    }

    /// The option the user picks when a command was run without its item
    /// (`docs/spec/cli.md#pickers`).
    pub fn pick<T>(
        &mut self,
        _checked: Picker,
        kind: PickKind,
        options: Vec<(String, T)>,
    ) -> Result<T, Error> {
        if options.is_empty() {
            return Err(Error::NothingToPick { kind });
        }

        let (labels, values): (Vec<String>, Vec<T>) = options.into_iter().unzip();
        let index = self
            .console
            .pick(&format!("Choose {}:", kind.prompt()), &labels)?;
        values.into_iter().nth(index).ok_or(Error::Cancelled)
    }
}
