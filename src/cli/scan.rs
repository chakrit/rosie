//! `scan` / `plan`, `clean`, and `run` (`docs/spec/cli.md#scan-and-clean`,
//! `docs/spec/plan.md`).

use std::path::{Path, PathBuf};

use super::args::{Format, Mode, Place, PlanSource, ScanArgs, Target, TreeArgs, Verb};
use super::console::{Console, Finish, PromptError};
use super::findings::{findings_lines, problem_line, stats_lines};
use super::plan_list::{LeftOut, plan_listing};
use super::session::{PickKind, Session};
use super::{Error, ExitStatus};
use crate::apps::{self, AppScanner};
use crate::config::Config;
use crate::fs::{Backend, FsContext, FsOp, UserInput};
use crate::output::Mark;
use crate::packs::Network;
use crate::plan::{ItemSize, Plan, PlanStats, Selection, Sides};
use crate::run::{Elevation, Outcome, Runner, SkipReason};
use crate::scan::{self, Refused, Scan, WalkSkips};

/// What the scan behind a run skipped, which the end-of-run stats count beside the
/// run's own items (`docs/spec/plan.md#stats`). A saved plan's run has none.
#[derive(Debug, Clone, Copy, Default)]
struct ScanSkips<'s> {
    walk: WalkSkips,
    /// Items left out of the plan because a process executes from them.
    refused: &'s [Refused],
}

const RUNNING_PROCESS: Outcome = Outcome::Skipped(SkipReason::RunningProcess);

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

const PROMPT: &str = "Clean the items marked clean? [y/N/e]";

/// The two choices for a rule conflict; the first makes it `clean`.
const CLEAN: &str = "clean: remove it";
const SKIP: &str = "skip: leave it";

/// How resolving the rule conflicts ended.
enum Resolved {
    Plan(Plan),
    /// The user cancelled a choice; the plan stays as it was.
    Cancelled,
}

/// How the `clean` confirmation ended.
enum Confirmation {
    /// The user answered `y`: run this plan, trimmed by any `e` edits.
    Run(Plan),
    /// Nothing is `clean`, so nothing was asked.
    NothingClean,
    /// The user answered `N`, or cancelled the prompt.
    Declined,
}

impl<'a, B: Backend + Sync, N: Network, C: Console> Session<'a, B, N, C> {
    pub fn scan(&mut self, args: ScanArgs) -> Result<ExitStatus, Error> {
        let by_terminal = match self.console.terminals().stdout {
            true => Format::List,
            false => Format::Toml,
        };
        let format = args.format.or(by_terminal)?;
        let config = self.config()?;
        let mode = args.target.mode();
        let scan = self
            .find(&config, &mode, "rosie scan app <X.app>")
            .map_err(|error| named_nothing(error, &args.target, Place::Mode(Verb::Scan)))?;
        let plan = answered(&scan.plan, mode.flags().conflicts.conflicts);

        let left_out = LeftOut {
            skipped: &scan.skipped,
            refused: &scan.refused,
            mode: &mode,
        };
        match format {
            Format::List => {
                self.show_plan(&plan, &left_out)?;
                self.show_problems(&scan)?;
            }
            Format::Toml => {
                self.say(plan.to_toml()?.trim_end())?;
                self.show_findings(&scan, &mode)?;
            }
            Format::Sh => {
                self.say(plan.to_sh()?.trim_end())?;
                self.show_findings(&scan, &mode)?;
            }
        }
        self.show_stats(&plan, scan.skip_counts())?;
        Ok(ExitStatus::failed_if(!scan.problems.is_empty()))
    }

    /// Scans, shows the plan, and runs it once the user answers `y`; `e` trims it first.
    pub fn clean(&mut self, target: Target, place: Place) -> Result<ExitStatus, Error> {
        if !self.console.terminals().interactive() {
            return Err(Error::Usage(
                "rosie clean asks for confirmation and needs a terminal; save a plan with \
                 rosie scan … > plan.toml, then rosie run plan.toml"
                    .to_owned(),
            ));
        }
        let config = self.config()?;
        let mode = target.mode();
        let scan = self
            .find(&config, &mode, "rosie clean app <X.app>")
            .map_err(|error| named_nothing(error, &target, place))?;
        let skips = ScanSkips {
            walk: scan.skip_counts(),
            refused: &scan.refused,
        };
        let errored = !scan.problems.is_empty();
        self.show_problems(&scan)?;

        let left_out = LeftOut {
            skipped: &scan.skipped,
            refused: &scan.refused,
            mode: &mode,
        };
        let plan = answered(&scan.plan, mode.flags().conflicts.conflicts);
        let plan = match self.confirm(plan, &left_out, skips.walk)? {
            Confirmation::Run(plan) => plan,
            Confirmation::NothingClean | Confirmation::Declined => {
                return Ok(ExitStatus::failed_if(errored));
            }
        };
        let failed = self.execute(&config, &plan, skips)?;
        Ok(ExitStatus::failed_if(failed || errored))
    }

    /// Runs a saved or piped plan without confirming it: the plan is the confirmation.
    /// Its conflicts take `answer` when given; on a terminal the user answers each one;
    /// with neither, nothing runs (`docs/spec/plan.md#rule-conflicts`).
    pub fn run_plan(
        &mut self,
        source: &PlanSource,
        answer: Option<Selection>,
    ) -> Result<ExitStatus, Error> {
        let bytes = match source {
            PlanSource::Stdin => self.console.read_stdin()?,
            PlanSource::File(path) => UserInput::read(&self.absolute(path)).perform(&self.own)?,
        };
        let text = str::from_utf8(&bytes).map_err(Error::PlanNotText)?;
        let plan = answered(&Plan::parse(text)?, answer);

        let lines: Vec<usize> = plan
            .conflicts()
            .into_iter()
            .filter_map(|(_, sides)| match sides {
                Sides::Read { line } => Some(*line),
                Sides::Scanned(_) => None,
            })
            .collect();
        let answered_all = plan.conflicts().is_empty();
        let plan = match (answered_all, self.console.terminals().interactive()) {
            (true, _) => plan,
            (false, true) => match self.resolve(&plan)? {
                Resolved::Plan(resolved) => resolved,
                Resolved::Cancelled => return Err(Error::Cancelled),
            },
            (false, false) => {
                return Err(Error::UnansweredConflicts {
                    plan: source.to_string(),
                    lines,
                });
            }
        };

        let config = self.config()?;
        let failed = self.execute(&config, &plan, ScanSkips::default())?;
        Ok(ExitStatus::failed_if(failed))
    }

    // scanning

    fn find(&mut self, config: &Config, mode: &Mode, app_usage: &str) -> Result<Scan, Error> {
        let settings = mode.flags().scan.settings(config.walk);

        match mode {
            Mode::Tree(TreeArgs { dir, rules, .. }) => {
                let scanner = self.rule_scanner(config, settings, rules)?;
                let dir = self.absolute(dir.as_deref().unwrap_or(Path::new(".")));
                let progress = self.console.progress();
                let scan = scanner.tree(&dir, &progress);
                progress.finish()?;
                Ok(scan?)
            }
            Mode::Caches { rules, .. } => {
                let scanner = self.rule_scanner(config, settings, rules)?;
                let progress = self.console.progress();
                let scan = scanner.caches(&progress);
                progress.finish()?;
                Ok(scan?)
            }
            Mode::App { app, .. } => {
                let ctx = self.roots_ctx(config)?;
                let app = match app {
                    Some(app) => self.absolute(app),
                    None => self.pick_app(&ctx, app_usage)?,
                };
                Ok(AppScanner::new(&ctx).scan(&app, &settings)?)
            }
            Mode::Orphans { .. } => {
                let ctx = self.roots_ctx(config)?;
                Ok(apps::scan_orphans(&ctx, &settings)?)
            }
        }
    }

    fn pick_app(&mut self, ctx: &FsContext<&'a B>, usage: &str) -> Result<PathBuf, Error> {
        let picker = self.picker(usage)?;
        let apps = apps::picker_apps(ctx)?;
        let options = apps
            .into_iter()
            .map(|app| (app.display().to_string(), app))
            .collect();
        self.pick(picker, PickKind::App, options)
    }

    // confirming and running

    /// Shows the plan and asks until the user answers `y` or `N`, or nothing is `clean`
    /// and nothing awaits a choice. `y` is refused while a rule conflict remains.
    fn confirm(
        &mut self,
        mut plan: Plan,
        left_out: &LeftOut<'_>,
        skips: WalkSkips,
    ) -> Result<Confirmation, Error> {
        loop {
            self.show_plan(&plan, left_out)?;
            self.show_stats(&plan, skips)?;
            let conflicts = plan.conflicts().len();
            if plan.clean_entries().is_empty() && conflicts == 0 {
                self.note(Mark::Note, "nothing to clean")?;
                return Ok(Confirmation::NothingClean);
            }

            let answer = match self.console.ask(PROMPT) {
                Ok(text) => Answer::parse(&text),
                Err(PromptError::Cancelled) => Answer::No,
                Err(error) => return Err(error.into()),
            };
            match (answer, conflicts) {
                (Answer::Yes, 0) => return Ok(Confirmation::Run(plan)),
                (Answer::Yes, _) => {
                    self.note(
                        Mark::Warning,
                        &format!("{conflicts} rule conflict(s) must be resolved first: answer e"),
                    )?;
                }
                (Answer::No, _) => {
                    self.note(Mark::Note, "nothing cleaned")?;
                    return Ok(Confirmation::Declined);
                }
                (Answer::Edit, _) => plan = self.edit(plan)?,
            }
        }
    }

    /// Asks the user to resolve each rule conflict, then shows the checklist of `clean`
    /// entries, all checked: entries the user unchecks leave the plan. A cancelled
    /// prompt leaves the plan as it was.
    fn edit(&mut self, plan: Plan) -> Result<Plan, Error> {
        let plan = match self.resolve(&plan)? {
            Resolved::Plan(resolved) => resolved,
            Resolved::Cancelled => return Ok(plan),
        };

        let kept = match self
            .console
            .checklist("Uncheck what to skip:", plan.clean_entries())
        {
            Ok(kept) => kept,
            Err(PromptError::Cancelled) => return Ok(plan),
            Err(error) => return Err(error.into()),
        };
        Ok(plan.keeping_clean_entries(&kept))
    }

    /// The plan with every conflict made `clean` or `skip` as the user picks it, one at a
    /// time (`docs/spec/plan.md#rule-conflicts`).
    fn resolve(&mut self, plan: &Plan) -> Result<Resolved, Error> {
        let options = [CLEAN.to_owned(), SKIP.to_owned()];

        let mut choices = Vec::new();
        for (conflict, sides) in plan.conflicts() {
            // A plan read back names the tiers only in a comment, so its rules stand in.
            let rules = match sides {
                Sides::Scanned(_) => sides.to_string(),
                Sides::Read { .. } => conflict.rules.join(", "),
            };
            let prompt = format!(
                "Rules disagree on {} ({}; {rules}):",
                conflict.path.display(),
                conflict.size,
            );
            let selection = match self.console.pick(&prompt, &options) {
                Ok(0) => Selection::Clean,
                Ok(_) => Selection::Skip,
                Err(PromptError::Cancelled) => return Ok(Resolved::Cancelled),
                Err(error) => return Err(error.into()),
            };
            choices.push((conflict, selection));
        }

        Ok(Resolved::Plan(plan.resolving(&choices)))
    }

    /// Runs the plan's `clean` entries through a gate bounded by the config's roots, and
    /// shows the end-of-run stats, the scan's skips among them. Returns whether any
    /// item failed.
    fn execute(
        &mut self,
        config: &Config,
        plan: &Plan,
        skips: ScanSkips<'_>,
    ) -> Result<bool, Error> {
        let ctx = self.roots_ctx(config)?;
        let elevation = Elevation {
            exe: self.env.exe.clone(),
        };

        let mut reporter = self.console.reporter();
        let mut stats = Runner::new(ctx).run(plan, &elevation, &mut reporter);
        reporter.finish()?;

        for refused in skips.refused {
            stats.record(ItemSize::Known(refused.size), RUNNING_PROCESS);
        }
        self.note(Mark::Note, stats_lines(stats, skips.walk).trim_end())?;
        Ok(stats.any_failed())
    }

    // output

    fn show_findings(&mut self, scan: &Scan, mode: &Mode) -> Result<(), Error> {
        for line in findings_lines(scan, mode) {
            self.note(line.mark, &line.text)?;
        }
        Ok(())
    }

    /// The scan's problems alone, for when the listing shows its skips.
    fn show_problems(&mut self, scan: &Scan) -> Result<(), Error> {
        for problem in &scan.problems {
            let line = problem_line(problem);
            self.note(line.mark, &line.text)?;
        }
        Ok(())
    }

    /// The plan listing on stdout (`docs/spec/plan.md#plan-listing`).
    fn show_plan(&mut self, plan: &Plan, left_out: &LeftOut<'_>) -> Result<(), Error> {
        let palette = self.console.stdout_palette();
        let width = self.console.stdout_width();
        for line in plan_listing(plan, left_out, &self.tilde(), palette, width) {
            self.say(&line)?;
        }
        Ok(())
    }

    fn show_stats(&mut self, plan: &Plan, skips: WalkSkips) -> Result<(), Error> {
        self.note(
            Mark::Note,
            stats_lines(PlanStats::of(plan), skips).trim_end(),
        )?;
        Ok(())
    }
}

/// `plan` with every conflict answered `answer`, or as it is with no answer given.
fn answered(plan: &Plan, answer: Option<Selection>) -> Plan {
    match answer {
        Some(answer) => plan.answering_conflicts(answer),
        None => plan.clone(),
    }
}

/// `error`, unless it says the folder `target` gives in place of a mode does not exist:
/// that word names no command, mode, or folder (`docs/spec/cli.md#modes`).
fn named_nothing(error: Error, target: &Target, place: Place) -> Error {
    match (error, target.folder_word()) {
        (Error::Scan(scan::Error::Missing { .. }), Some(word)) => Error::UnknownWord {
            word: word.to_path_buf(),
            place,
        },
        (error, _) => error,
    }
}
