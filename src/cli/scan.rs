//! `scan` / `plan`, `clean`, and `run` (`docs/spec/cli.md#scan-and-clean`,
//! `docs/spec/plan.md`).

use std::path::{Path, PathBuf};

use super::args::{CleanArgs, Mode, PlanSource, RuleFlags, ScanArgs, ScanFlags};
use super::console::{Console, Finish, PromptError};
use super::findings::{Findings, stats_lines};
use super::session::{PickKind, Session};
use super::{Error, ExitStatus};
use crate::apps;
use crate::config::{ConfigFile, Walk};
use crate::fs::{Backend, Gate};
use crate::packs::Network;
use crate::plan::{AggressiveItems, Plan, WalkSkips};
use crate::rules::RuleSet;
use crate::run::{Elevation, Runner};
use crate::scan::{Scanner, Settings};

/// How `scan` prints the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Toml,
    /// `--sh`: a shell script to inspect and run by hand.
    Sh,
}

/// The answer to the `clean` prompt (`docs/spec/plan.md#interactive-confirmation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Yes,
    No,
    Edit,
}

impl Answer {
    /// `N` is the default: anything but `y` or `e` cleans nothing.
    fn parse(text: &str) -> Answer {
        match text.trim().to_lowercase().as_str() {
            "y" | "yes" => Answer::Yes,
            "e" | "edit" => Answer::Edit,
            _ => Answer::No,
        }
    }
}

const PROMPT: &str = "Clean the ticked items? [y/N/e]";

impl<'a, B: Backend + Sync, N: Network, C: Console> Session<'a, B, N, C> {
    pub fn scan(&mut self, args: ScanArgs) -> Result<ExitStatus, Error> {
        let format = match args.sh {
            true => Format::Sh,
            false => Format::Toml,
        };
        let config = self.config()?;
        let findings = self.find(&config, &args.mode, &args.flags, "rosie scan app <X.app>")?;

        let text = match format {
            Format::Toml => findings.plan.to_toml()?,
            Format::Sh => findings.plan.to_sh(),
        };
        self.say(text.trim_end())?;
        self.show_findings(&findings, &args.mode)?;
        self.show_stats(&findings.plan, findings.skip_counts())?;
        Ok(ExitStatus::failed_if(findings.errored()))
    }

    /// Scans, shows the plan, and runs it once the user answers `y`; `e` trims it first.
    pub fn clean(&mut self, args: CleanArgs) -> Result<ExitStatus, Error> {
        if !self.console.terminals().interactive() {
            return Err(Error::Usage(
                "rosie clean asks for confirmation and needs a terminal; save a plan with \
                 rosie scan … > plan.toml, then rosie run plan.toml"
                    .to_owned(),
            ));
        }
        let config = self.config()?;
        let findings = self.find(&config, &args.mode, &args.flags, "rosie clean app <X.app>")?;
        let skips = findings.skip_counts();
        let errored = findings.errored();
        self.show_findings(&findings, &args.mode)?;

        let Some(plan) = self.confirm(findings.plan, skips)? else {
            return Ok(ExitStatus::failed_if(errored));
        };
        let failed = self.execute(&config, &plan, skips)?;
        Ok(ExitStatus::failed_if(failed || errored))
    }

    /// Runs a saved or piped plan without asking: the plan is the confirmation.
    pub fn run_plan(&mut self, source: &PlanSource) -> Result<ExitStatus, Error> {
        let bytes = match source {
            PlanSource::Stdin => self.console.read_stdin()?,
            PlanSource::File(path) => self.own.read_file(&self.absolute(path))?,
        };
        let text = str::from_utf8(&bytes).map_err(Error::PlanNotText)?;
        let plan = Plan::parse(text)?;

        let config = self.config()?;
        let failed = self.execute(&config, &plan, WalkSkips::default())?;
        Ok(ExitStatus::failed_if(failed))
    }

    // scanning

    fn find(
        &mut self,
        config: &ConfigFile,
        mode: &Mode,
        flags: &ScanFlags,
        app_usage: &str,
    ) -> Result<Findings, Error> {
        let walk = walk_with_mounts_flag(config.config().walk, flags);
        let aggressive = match flags.aggressive {
            true => AggressiveItems::Ticked,
            false => AggressiveItems::Unticked,
        };
        let gate = self.roots_gate(config.config())?;
        let home = &self.env.home;

        match mode {
            Mode::Tree { dir, rules } => {
                let walk = walk_with_rule_flags(walk, rules);
                let rules = self.rules(&rules.only)?;
                let dir = self.absolute(dir.as_deref().unwrap_or(Path::new(".")));
                let scanner = self.scanner(gate, rules, walk, aggressive);
                let progress = self.console.progress();
                let scan = scanner.tree(&dir, &progress);
                progress.finish()?;
                Ok(scan?.into())
            }
            Mode::Caches { rules } => {
                let walk = walk_with_rule_flags(walk, rules);
                let rules = self.rules(&rules.only)?;
                let scanner = self.scanner(gate, rules, walk, aggressive);
                let progress = self.console.progress();
                let scan = scanner.caches(&progress);
                progress.finish()?;
                Ok(scan?.into())
            }
            Mode::App { app } => {
                let app = match app {
                    Some(app) => self.absolute(app),
                    None => self.pick_app(&gate, app_usage)?,
                };
                let home = &self.env.home;
                Ok(apps::scan_app(&gate, &app, home, aggressive, walk.into())?.into())
            }
            Mode::Orphans => Ok(apps::scan_orphans(&gate, home, aggressive, walk.into())?.into()),
        }
    }

    fn scanner(
        &self,
        gate: Gate<&'a B>,
        rules: RuleSet,
        walk: Walk,
        aggressive: AggressiveItems,
    ) -> Scanner<&'a B> {
        let settings = Settings {
            home: self.env.home.clone(),
            walk,
            aggressive,
        };
        Scanner::new(gate, rules, settings)
    }

    fn pick_app(&mut self, gate: &Gate<&'a B>, usage: &str) -> Result<PathBuf, Error> {
        let picker = self.picker(usage)?;
        let apps = apps::picker_apps(gate, &self.env.home)?;
        let options = apps
            .into_iter()
            .map(|app| (app.display().to_string(), app))
            .collect();
        self.pick(picker, PickKind::App, options)
    }

    // confirming and running

    /// Shows the plan and asks until the user answers `y` or `N`. Returns the plan to
    /// run, trimmed by any `e` edits, or `None` when nothing is to run.
    fn confirm(&mut self, mut plan: Plan, skips: WalkSkips) -> Result<Option<Plan>, Error> {
        loop {
            self.say(plan.to_toml()?.trim_end())?;
            self.show_stats(&plan, skips)?;
            if plan.ticked().is_empty() {
                self.note("nothing to clean")?;
                return Ok(None);
            }

            let answer = match self.console.ask(PROMPT) {
                Ok(text) => Answer::parse(&text),
                Err(PromptError::Cancelled) => Answer::No,
                Err(error) => return Err(error.into()),
            };
            match answer {
                Answer::Yes => return Ok(Some(plan)),
                Answer::No => {
                    self.note("nothing cleaned")?;
                    return Ok(None);
                }
                Answer::Edit => plan = self.edit(plan)?,
            }
        }
    }

    /// The pre-ticked checklist: entries the user unticks leave the plan. A cancelled
    /// checklist leaves the plan as it was.
    fn edit(&mut self, plan: Plan) -> Result<Plan, Error> {
        let kept = match self
            .console
            .checklist("Untick what to keep:", plan.ticked())
        {
            Ok(kept) => kept,
            Err(PromptError::Cancelled) => return Ok(plan),
            Err(error) => return Err(error.into()),
        };
        Ok(plan.keeping_ticked(&kept))
    }

    /// Runs the plan's ticked entries through a gate bounded by the config's roots, and
    /// shows the end-of-run stats. Returns whether any item failed.
    fn execute(
        &mut self,
        config: &ConfigFile,
        plan: &Plan,
        skips: WalkSkips,
    ) -> Result<bool, Error> {
        let gate = self.roots_gate(config.config())?;
        let elevation = Elevation {
            exe: self.env.exe.clone(),
            home: self.env.home.clone(),
        };

        let mut reporter = self.console.reporter();
        let mut stats = Runner::new(gate).run(plan, &elevation, &mut reporter);
        reporter.finish()?;

        stats.walk_skips = skips;
        self.note(stats.to_string().trim_end())?;
        Ok(stats.any_failed())
    }

    // output

    fn show_findings(&mut self, findings: &Findings, mode: &Mode) -> Result<(), Error> {
        for line in findings.lines(mode) {
            self.note(&line)?;
        }
        Ok(())
    }

    fn show_stats(&mut self, plan: &Plan, skips: WalkSkips) -> Result<(), Error> {
        self.note(stats_lines(plan, skips).trim_end())?;
        Ok(())
    }
}

/// The config's `[walk]` settings, with `enter_mounts` turned on by `--enter-mounts`
/// (`docs/spec/safety.md#walk-skips`).
fn walk_with_mounts_flag(config: Walk, flags: &ScanFlags) -> Walk {
    Walk {
        enter_mounts: config.enter_mounts || flags.enter_mounts,
        ..config
    }
}

/// `walk`, with `enter_bundles` and `enter_placeholders` each turned on by its CLI flag.
fn walk_with_rule_flags(walk: Walk, flags: &RuleFlags) -> Walk {
    Walk {
        enter_bundles: walk.enter_bundles || flags.enter_bundles,
        enter_placeholders: walk.enter_placeholders || flags.enter_placeholders,
        ..walk
    }
}
