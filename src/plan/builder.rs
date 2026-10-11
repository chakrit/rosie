//! Builds a plan from what a scan found (`docs/spec/plan.md#plan-file`).
//!
//! - Path entries are keyed by path and list every rule that matched that path.
//! - Nested matches collapse into the outer one, which takes over their launch-job
//!   bootouts and runs elevated when any of them would.
//! - Each tool rule's command is its own entry: tool commands are never compared.
//! - Aggressive items are `keep` unless `--aggressive` is given, which also makes a
//!   rule's `cmd_aggressive` replace its `cmd`; items outside the roots are
//!   `outside-roots`.
//! - Rules that disagree on the tier of a path, or of paths nested in it, make the entry
//!   a conflict (`docs/spec/plan.md#rule-conflicts`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::conflict::{Tiers, Verdict};
use super::deletes::{Found, Located};
use super::{
    Bootout, Delete, Deletes, Error, ItemKind, LaunchDomain, Package, Plan, Receipt, Report, RunAs,
    Selection, Size, Status, Tool, check_path,
};
use crate::fs::ShellCommand;

/// Whether `--aggressive` was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggressiveItems {
    Keep,
    Clean,
}

/// Which of a rule's twin fields matched (`docs/spec/rules.md#aggressive-twins`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Twin {
    Normal,
    Aggressive,
}

/// A tool rule's commands, as its `cmd` and `cmd_aggressive` fields give them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCmds<'a> {
    Normal(&'a ShellCommand),
    Aggressive(&'a ShellCommand),
    Twins {
        cmd: &'a ShellCommand,
        cmd_aggressive: &'a ShellCommand,
    },
}

/// Whether a path lies within the `roots` allowlist, as the roots gate decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    InRoots,
    OutsideRoots,
}

/// One rule matching one path during a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMatch {
    pub path: PathBuf,
    pub kind: ItemKind,
    pub size: Size,
    pub run_as: RunAs,
    pub reach: Reach,
    pub rule: String,
    pub twin: Twin,
}

/// What [`PlanBuilder::build`] makes: the plan, and the tier its rules agree on for each
/// delete entry that is not a conflict, which the plan file does not record.
#[derive(Debug)]
pub struct Built {
    pub plan: Plan,
    pub tiers: AgreedTiers,
}

/// The tier each delete entry's rules, and the rules of the paths nested in it, agree
/// on, by the entry's path. A conflict entry has none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgreedTiers(BTreeMap<PathBuf, Twin>);

impl AgreedTiers {
    pub fn of(&self, path: &Path) -> Option<Twin> {
        self.0.get(path).copied()
    }
}

pub struct PlanBuilder {
    aggressive: AggressiveItems,
    deletes: BTreeMap<PathBuf, FoundPath>,
    tools: Vec<Tool>,
    receipts: Vec<Receipt>,
    reports: Vec<Report>,
}

impl PlanBuilder {
    pub fn new(aggressive: AggressiveItems) -> Self {
        PlanBuilder {
            aggressive,
            deletes: BTreeMap::new(),
            tools: Vec::new(),
            receipts: Vec::new(),
            reports: Vec::new(),
        }
    }

    /// Adds a matched file or folder. A path matched again gains the rule.
    pub fn add_path(&mut self, found: PathMatch) -> Result<(), Error> {
        self.add_delete(found).map(|_| ())
    }

    /// Adds a launch job's plist: its delete, carrying the job's bootout. A plist added
    /// again keeps its one bootout; a second domain for it is refused.
    pub fn add_launch_job(&mut self, plist: PathMatch, domain: LaunchDomain) -> Result<(), Error> {
        // Until `build` collapses nested entries, an entry holds only its own plist's bootout.
        let known = self
            .deletes
            .get(plist.path.as_path())
            .and_then(|entry| entry.bootouts.first());
        if let Some(bootout) = known
            && bootout.domain != domain
        {
            return Err(Error::ConflictingLaunchDomain {
                plist: bootout.plist.clone(),
                first: bootout.domain,
                second: domain,
            });
        }

        let entry = self.add_delete(plist)?;
        if entry.bootouts.is_empty() {
            let plist = entry.path.clone();
            entry.bootouts.push(Bootout { plist, domain });
        }
        Ok(())
    }

    /// Adds a tool rule's commands, each its own entry keyed by the rule. Without
    /// `--aggressive` its `cmd` is `clean` and its `cmd_aggressive` listed `keep`; with
    /// it, `cmd_aggressive` replaces `cmd`.
    pub fn add_tool(&mut self, rule: &str, cmds: ToolCmds<'_>) {
        let fields = match (cmds, self.aggressive) {
            (ToolCmds::Normal(cmd), _) => vec![(cmd, Twin::Normal)],
            (ToolCmds::Aggressive(cmd), _) => vec![(cmd, Twin::Aggressive)],
            (ToolCmds::Twins { cmd_aggressive, .. }, AggressiveItems::Clean) => {
                vec![(cmd_aggressive, Twin::Aggressive)]
            }
            (
                ToolCmds::Twins {
                    cmd,
                    cmd_aggressive,
                },
                AggressiveItems::Keep,
            ) => vec![(cmd, Twin::Normal), (cmd_aggressive, Twin::Aggressive)],
        };

        let tools: Vec<Tool> = fields
            .into_iter()
            .map(|(command, twin)| Tool {
                rule: rule.to_owned(),
                command: command.clone(),
                selection: self.selection(twin),
            })
            .collect();
        self.tools.extend(tools);
    }

    pub fn add_receipt(&mut self, package: &str, twin: Twin) -> Result<(), Error> {
        let package = Package::new(package.to_owned())?;

        let selection = self.selection(twin);
        self.receipts.push(Receipt { package, selection });
        Ok(())
    }

    pub fn add_report(&mut self, report: Report) {
        self.reports.push(report);
    }

    pub fn build(self) -> Built {
        let aggressive = self.aggressive;
        let mut agreed = BTreeMap::new();
        let deletes = Deletes::collapsing(self.deletes, |found| {
            let verdict = found.tiers.verdict();
            if let Verdict::Agreed(twin) = &verdict {
                agreed.insert(found.path.clone(), *twin);
            }
            found.into_delete(verdict, aggressive)
        });

        let plan = Plan {
            deletes,
            tools: self.tools,
            receipts: self.receipts,
            reports: self.reports,
        };
        Built {
            plan,
            tiers: AgreedTiers(agreed),
        }
    }

    // entries

    fn add_delete(&mut self, found: PathMatch) -> Result<&mut FoundPath, Error> {
        check_path(&found.path)?;

        let path = found.path;
        let entry = self.deletes.entry(path.clone()).or_insert(FoundPath {
            path,
            kind: found.kind,
            size: found.size,
            rules: Vec::new(),
            tiers: Tiers::of(found.rule.clone(), found.twin),
            reach: found.reach,
            run_as: found.run_as,
            bootouts: Vec::new(),
        });

        entry.tiers.add(found.rule.clone(), found.twin);
        if !entry.rules.contains(&found.rule) {
            entry.rules.push(found.rule);
            entry.rules.sort();
        }
        Ok(entry)
    }

    // selection

    fn selection(&self, twin: Twin) -> Selection {
        self.aggressive.selection(twin)
    }
}

impl AggressiveItems {
    fn selection(self, twin: Twin) -> Selection {
        match (twin, self) {
            (Twin::Normal, _) => Selection::Clean,
            (Twin::Aggressive, AggressiveItems::Clean) => Selection::Clean,
            (Twin::Aggressive, AggressiveItems::Keep) => Selection::Keep,
        }
    }
}

/// What a scan found at one path: the rules that matched it, and the tiers of those and
/// of the rules matching paths nested in it.
struct FoundPath {
    path: PathBuf,
    kind: ItemKind,
    size: Size,
    /// The rules that matched this path itself, sorted.
    rules: Vec<String>,
    tiers: Tiers,
    reach: Reach,
    run_as: RunAs,
    bootouts: Vec<Bootout>,
}

impl FoundPath {
    /// `outside-roots` outside the roots; otherwise the tier its rules agree on, or a
    /// conflict, as `verdict`, the verdict of its tiers, says.
    fn into_delete(self, verdict: Verdict, aggressive: AggressiveItems) -> Delete {
        let status = match (self.reach, verdict) {
            (Reach::OutsideRoots, _) => Status::OutsideRoots,
            (Reach::InRoots, Verdict::Agreed(twin)) => aggressive.selection(twin).into(),
            (Reach::InRoots, Verdict::Disagreed(disagreement)) => Status::Conflict(disagreement),
        };

        Delete {
            path: self.path,
            kind: self.kind,
            size: self.size,
            rules: self.rules,
            status,
            run_as: self.run_as,
            bootouts: self.bootouts,
        }
    }
}

impl Located for FoundPath {
    fn path(&self) -> &Path {
        &self.path
    }
}

/// The outer path keeps its own rules, size, and reach. It takes over the bootouts of
/// the paths inside it, so their jobs are unloaded exactly when it is deleted, runs
/// elevated when any of them would, and weighs their rules' tiers with its own.
impl Found for FoundPath {
    fn absorb(&mut self, inner: FoundPath) {
        self.bootouts.extend(inner.bootouts);
        if inner.run_as == RunAs::Sudo {
            self.run_as = RunAs::Sudo;
        }
        self.tiers.absorb(inner.tiers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Runnable, tool_command};

    fn found(path: &str, rule: &str, twin: Twin) -> PathMatch {
        PathMatch {
            path: PathBuf::from(path),
            kind: ItemKind::Folder,
            size: Size::bytes(1000),
            run_as: RunAs::User,
            reach: Reach::InRoots,
            rule: rule.to_owned(),
            twin,
        }
    }

    fn plist(path: &str, twin: Twin) -> PathMatch {
        PathMatch {
            kind: ItemKind::File,
            ..found(path, "app/x", twin)
        }
    }

    fn built(aggressive: AggressiveItems, matches: Vec<PathMatch>) -> Plan {
        let mut builder = PlanBuilder::new(aggressive);
        for found in matches {
            builder.add_path(found).expect("valid match");
        }
        builder.build().plan
    }

    fn paths(plan: &Plan) -> Vec<&str> {
        plan.deletes()
            .iter()
            .map(|delete| delete.path.to_str().expect("utf-8"))
            .collect()
    }

    /// The plists a run boots out: the user's part, then the elevated part.
    fn runnable_bootouts(plan: &Plan) -> Vec<&str> {
        [RunAs::User, RunAs::Sudo]
            .into_iter()
            .flat_map(|run_as| {
                plan.runnable_as(run_as)
                    .bootouts()
                    .map(|bootout| bootout.plist.to_str().expect("utf-8"))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn tool_summary(plan: &Plan) -> Vec<(&str, &str, Selection)> {
        plan.tools()
            .iter()
            .map(|tool| (tool.rule.as_str(), tool.command.cmd(), tool.selection))
            .collect()
    }

    fn rules(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn keys_paths_and_lists_every_rule_that_matched() {
        let plan = built(
            AggressiveItems::Keep,
            vec![
                found("/w/app/node_modules", "rosie/node", Twin::Normal),
                found("/w/app/node_modules", "user/js", Twin::Normal),
                found("/w/app/node_modules", "rosie/node", Twin::Normal),
            ],
        );

        let [delete] = plan.deletes() else {
            panic!("one entry expected: {plan:?}");
        };
        assert_eq!(delete.rules, ["rosie/node", "user/js"]);
    }

    #[test]
    fn nested_matches_collapse_into_the_outer_one() {
        let plan = built(
            AggressiveItems::Keep,
            vec![
                found("/w/app/target/debug/build", "rosie/x", Twin::Normal),
                found("/w/app/target", "rosie/cargo", Twin::Normal),
                found("/w/app/target/debug", "rosie/y", Twin::Normal),
                found("/w/app/targets", "rosie/z", Twin::Normal),
                found("/w/app 2/target", "rosie/cargo", Twin::Normal),
            ],
        );

        assert_eq!(
            paths(&plan),
            ["/w/app/target", "/w/app/targets", "/w/app 2/target"]
        );
        assert_eq!(plan.deletes()[0].rules, ["rosie/cargo"]);
    }

    fn conflict(normal: &[&str], aggressive: &[&str]) -> (Vec<String>, Vec<String>) {
        (rules(normal), rules(aggressive))
    }

    fn disagreement(status: &Status) -> (Vec<String>, Vec<String>) {
        let Status::Conflict(disagreement) = status else {
            panic!("expected a conflict, got {status:?}");
        };
        (
            disagreement.normal().to_vec(),
            disagreement.aggressive().to_vec(),
        )
    }

    #[test]
    fn rules_disagreeing_across_nested_paths_make_the_outer_entry_a_conflict() {
        let plan = built(
            AggressiveItems::Keep,
            vec![
                found("/w/app/.venv", "rosie/venv", Twin::Aggressive),
                found(
                    "/w/app/.venv/lib/__pycache__",
                    "rosie/pycache",
                    Twin::Normal,
                ),
            ],
        );

        assert_eq!(paths(&plan), ["/w/app/.venv"]);
        assert_eq!(
            disagreement(&plan.deletes()[0].status),
            conflict(&["rosie/pycache"], &["rosie/venv"])
        );
    }

    #[test]
    fn rules_agreeing_across_nested_paths_keep_their_tier() {
        let plan = built(
            AggressiveItems::Keep,
            vec![
                found("/w/app/.venv", "rosie/venv", Twin::Aggressive),
                found("/w/app/.venv/lib", "alice/venv", Twin::Aggressive),
            ],
        );

        assert_eq!(plan.deletes()[0].status, Status::Keep);
    }

    #[test]
    fn an_outer_entry_runs_elevated_when_a_collapsed_one_would() {
        let plan = built(
            AggressiveItems::Keep,
            vec![
                found("/w/app/build", "rosie/build", Twin::Normal),
                PathMatch {
                    run_as: RunAs::Sudo,
                    ..found("/w/app/build/root-owned", "rosie/x", Twin::Normal)
                },
                found("/w/other", "rosie/build", Twin::Normal),
            ],
        );

        let run_as: Vec<RunAs> = plan.deletes().iter().map(|d| d.run_as).collect();
        assert_eq!(run_as, [RunAs::Sudo, RunAs::User]);
    }

    #[test]
    fn aggressive_items_are_keep_unless_aggressive() {
        let matches = || {
            vec![
                found("/w/a", "rosie/a", Twin::Aggressive),
                found("/w/b", "rosie/b", Twin::Normal),
            ]
        };
        let statuses = |plan: Plan| -> Vec<Status> {
            plan.deletes()
                .iter()
                .map(|delete| delete.status.clone())
                .collect()
        };

        let default = built(AggressiveItems::Keep, matches());
        let aggressive = built(AggressiveItems::Clean, matches());

        assert_eq!(statuses(default), [Status::Keep, Status::Clean]);
        assert_eq!(statuses(aggressive), [Status::Clean, Status::Clean]);
    }

    #[test]
    fn rules_disagreeing_on_one_path_make_it_a_conflict_even_with_aggressive() {
        for aggressive in [AggressiveItems::Keep, AggressiveItems::Clean] {
            let plan = built(
                aggressive,
                vec![
                    found("/w/a", "rosie/strict", Twin::Aggressive),
                    found("/w/a", "rosie/plain", Twin::Normal),
                ],
            );

            assert_eq!(
                disagreement(&plan.deletes()[0].status),
                conflict(&["rosie/plain"], &["rosie/strict"])
            );
        }
    }

    #[test]
    fn one_rule_matching_a_path_at_both_tiers_is_not_a_conflict() {
        let plan = built(
            AggressiveItems::Keep,
            vec![
                found("/w/a", "rosie/a", Twin::Aggressive),
                found("/w/a", "rosie/a", Twin::Normal),
            ],
        );

        assert_eq!(plan.deletes()[0].status, Status::Clean);
    }

    #[test]
    fn an_outside_roots_path_stays_outside_roots_whatever_its_rules_say() {
        let outside = |rule, twin| PathMatch {
            reach: Reach::OutsideRoots,
            ..found("/elsewhere/cache", rule, twin)
        };

        let plan = built(
            AggressiveItems::Keep,
            vec![
                outside("rosie/a", Twin::Normal),
                outside("alice/b", Twin::Aggressive),
            ],
        );

        assert_eq!(plan.deletes()[0].status, Status::OutsideRoots);
    }

    #[test]
    fn items_outside_roots_are_outside_roots_even_when_aggressive() {
        let outside = |twin| PathMatch {
            reach: Reach::OutsideRoots,
            ..found("/elsewhere/cache", "rosie/c", twin)
        };

        let plan = built(
            AggressiveItems::Clean,
            vec![outside(Twin::Normal), outside(Twin::Aggressive)],
        );

        assert_eq!(plan.deletes()[0].status, Status::OutsideRoots);
    }

    #[test]
    fn refuses_relative_paths() {
        let mut builder = PlanBuilder::new(AggressiveItems::Keep);

        let refused = builder.add_path(found("w/app/target", "rosie/cargo", Twin::Normal));

        assert!(matches!(refused, Err(Error::RelativePath { .. })));
    }

    /// `/w/app/../../etc` names `/etc`, and would collapse into `/w/app` as if inside it.
    #[test]
    fn refuses_dot_and_dot_dot_components() {
        let dotted = ["/w/app/../../etc", "/w/./app", "/w/app/.", "/w/.."];

        for path in dotted {
            let mut builder = PlanBuilder::new(AggressiveItems::Keep);

            let refusals = [
                builder.add_path(found(path, "rosie/x", Twin::Normal)),
                builder.add_launch_job(plist(path, Twin::Normal), LaunchDomain::System),
            ];

            for refused in refusals {
                assert!(
                    matches!(refused, Err(Error::DotComponent { .. })),
                    "{path}: {refused:?}"
                );
            }
            assert_eq!(builder.build().plan, Plan::default(), "{path}");
        }
    }

    /// `rosie run` must be able to read back every plan a scan writes, and the parser
    /// refuses an empty package or subject.
    #[test]
    fn refuses_an_empty_receipt_package_or_report_subject() {
        let mut builder = PlanBuilder::new(AggressiveItems::Keep);

        let package = builder.add_receipt("", Twin::Normal);
        let report = Report::new(String::new(), vec!["Remove X".to_owned()]);

        assert!(matches!(package, Err(Error::EmptyPackage)), "{package:?}");
        assert!(matches!(report, Err(Error::EmptySubject)), "{report:?}");
        assert_eq!(builder.build().plan, Plan::default());
    }

    /// A NUL byte ends a path or an argument at the operating system, so the item run
    /// would not be the item listed.
    #[test]
    fn refuses_a_nul_byte_in_any_path_or_argument() {
        let mut builder = PlanBuilder::new(AggressiveItems::Keep);

        let refusals = [
            builder.add_path(found("/w/a\0b", "rosie/cargo", Twin::Normal)),
            builder.add_launch_job(
                found("/w/x\0.plist", "rosie/launch", Twin::Normal),
                LaunchDomain::System,
            ),
            builder.add_receipt("com.x\0.pkg", Twin::Normal),
        ];

        for refused in refusals {
            assert!(matches!(refused, Err(Error::NulByte { .. })), "{refused:?}");
        }
        let plan = builder.build().plan;
        assert_eq!(plan, Plan::default());
    }

    // launch jobs

    #[test]
    fn a_launch_job_boots_out_with_its_plists_status() {
        let mut builder = PlanBuilder::new(AggressiveItems::Keep);
        builder
            .add_launch_job(
                plist("/L/LaunchAgents/com.x.plist", Twin::Aggressive),
                LaunchDomain::Gui(501),
            )
            .expect("valid plist");
        builder
            .add_launch_job(
                plist("/L/LaunchDaemons/com.y.plist", Twin::Normal),
                LaunchDomain::System,
            )
            .expect("valid plist");

        let plan = builder.build().plan;

        assert_eq!(runnable_bootouts(&plan), ["/L/LaunchDaemons/com.y.plist"]);
        assert_eq!(
            plan.deletes()[0].bootouts,
            [Bootout {
                plist: PathBuf::from("/L/LaunchAgents/com.x.plist"),
                domain: LaunchDomain::Gui(501),
            }]
        );
    }

    // A user's folder holding a `system` job's plist belongs to the elevated part: the
    // user's process neither boots that job out, which needs root, nor deletes the
    // folder before the elevated part has.
    #[test]
    fn a_user_runnable_never_carries_a_system_bootout() {
        let mut builder = PlanBuilder::new(AggressiveItems::Keep);
        builder
            .add_path(found("/U/app", "app/x", Twin::Normal))
            .expect("valid match");
        builder
            .add_launch_job(
                plist("/U/app/com.y.plist", Twin::Normal),
                LaunchDomain::System,
            )
            .expect("valid plist");
        builder
            .add_launch_job(
                plist("/U/agents/com.x.plist", Twin::Normal),
                LaunchDomain::Gui(501),
            )
            .expect("valid plist");
        let plan = builder.build().plan;

        let user = plan.runnable_as(RunAs::User);
        let sudo = plan.runnable_as(RunAs::Sudo);

        let plists = |runnable: &Runnable| -> Vec<PathBuf> {
            runnable.bootouts().map(|b| b.plist.clone()).collect()
        };
        let deleted = |runnable: &Runnable| -> Vec<PathBuf> {
            runnable.deletes().iter().map(|d| d.path.clone()).collect()
        };
        assert_eq!(plists(&user), [PathBuf::from("/U/agents/com.x.plist")]);
        assert_eq!(deleted(&user), [PathBuf::from("/U/agents/com.x.plist")]);
        assert_eq!(plists(&sudo), [PathBuf::from("/U/app/com.y.plist")]);
        assert_eq!(deleted(&sudo), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_plist_matched_twice_is_booted_out_once_with_its_final_status() {
        let mut builder = PlanBuilder::new(AggressiveItems::Keep);
        let path = "/L/LaunchAgents/com.x.plist";
        builder
            .add_launch_job(plist(path, Twin::Aggressive), LaunchDomain::Gui(501))
            .expect("valid plist");
        builder
            .add_launch_job(plist(path, Twin::Normal), LaunchDomain::Gui(501))
            .expect("valid plist");

        let plan = builder.build().plan;

        assert_eq!(plan.deletes()[0].bootouts.len(), 1, "{plan:?}");
        assert_eq!(runnable_bootouts(&plan), [path]);
    }

    #[test]
    fn a_plist_marked_clean_by_a_plain_path_match_is_still_booted_out() {
        let mut builder = PlanBuilder::new(AggressiveItems::Keep);
        let path = "/L/LaunchAgents/com.x.plist";
        builder
            .add_launch_job(plist(path, Twin::Aggressive), LaunchDomain::Gui(501))
            .expect("valid plist");
        builder
            .add_path(plist(path, Twin::Normal))
            .expect("valid path");

        let plan = builder.build().plan;

        assert_eq!(runnable_bootouts(&plan), [path]);
    }

    #[test]
    fn a_plist_inside_a_clean_folder_is_booted_out_before_the_folder_goes() {
        let mut builder = PlanBuilder::new(AggressiveItems::Keep);
        let path = "/w/app/support/com.x.plist";
        builder
            .add_launch_job(plist(path, Twin::Aggressive), LaunchDomain::Gui(501))
            .expect("valid plist");
        builder
            .add_path(found("/w/app", "app/x", Twin::Normal))
            .expect("valid path");

        let plan = builder.build().plan;

        assert_eq!(paths(&plan), ["/w/app"]);
        assert_eq!(runnable_bootouts(&plan), [path]);
    }

    #[test]
    fn refuses_a_plist_booted_out_of_two_domains() {
        let mut builder = PlanBuilder::new(AggressiveItems::Keep);
        let path = "/L/LaunchAgents/com.x.plist";
        builder
            .add_launch_job(plist(path, Twin::Normal), LaunchDomain::Gui(501))
            .expect("valid plist");

        let refused = builder.add_launch_job(plist(path, Twin::Normal), LaunchDomain::System);

        assert!(
            matches!(refused, Err(Error::ConflictingLaunchDomain { .. })),
            "{refused:?}"
        );
    }

    // tools

    /// Rosie never compares tool commands (`docs/spec/plan.md#plan-file`): two rules
    /// with byte-identical commands are two actions, each with its own status.
    #[test]
    fn each_rules_command_is_its_own_entry_even_when_identical() {
        let mut builder = PlanBuilder::new(AggressiveItems::Keep);
        let prune = tool_command("docker system prune --force");
        let prune_all = tool_command("docker system prune --all --force");
        let cleanup = tool_command("brew cleanup");
        let adds = [
            (
                "rosie/docker",
                ToolCmds::Twins {
                    cmd: &prune,
                    cmd_aggressive: &prune_all,
                },
            ),
            ("alice/docker", ToolCmds::Aggressive(&prune)),
            ("rosie/brew", ToolCmds::Normal(&cleanup)),
        ];
        for (rule, cmds) in adds {
            builder.add_tool(rule, cmds);
        }

        let plan = builder.build().plan;

        assert_eq!(
            tool_summary(&plan),
            [
                (
                    "rosie/docker",
                    "docker system prune --force",
                    Selection::Clean
                ),
                (
                    "rosie/docker",
                    "docker system prune --all --force",
                    Selection::Keep,
                ),
                (
                    "alice/docker",
                    "docker system prune --force",
                    Selection::Keep
                ),
                ("rosie/brew", "brew cleanup", Selection::Clean),
            ]
        );
    }

    #[test]
    fn with_aggressive_cmd_aggressive_replaces_cmd() {
        let mut builder = PlanBuilder::new(AggressiveItems::Clean);
        let prune = tool_command("docker system prune --force");
        let prune_all = tool_command("docker system prune --all --force");
        let cleanup = tool_command("brew cleanup");
        let twins = ToolCmds::Twins {
            cmd: &prune,
            cmd_aggressive: &prune_all,
        };
        builder.add_tool("rosie/docker", twins);
        builder.add_tool("rosie/brew", ToolCmds::Normal(&cleanup));

        let plan = builder.build().plan;

        assert_eq!(
            tool_summary(&plan),
            [
                (
                    "rosie/docker",
                    "docker system prune --all --force",
                    Selection::Clean,
                ),
                ("rosie/brew", "brew cleanup", Selection::Clean),
            ]
        );
    }
}
