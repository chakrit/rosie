//! Builds a plan from what a scan found (`docs/spec/plan.md#plan-file`).
//!
//! - Path entries are keyed by path and list every rule that matched that path. Each
//!   path is an [`IdlePath`], one the process table admitted: no plan this builder
//!   makes holds an item a process executes from
//!   (`docs/spec/safety.md#running-processes`).
//! - Nested matches collapse into the outer one, which takes over their launch-job
//!   bootouts and runs elevated when any of them would.
//! - Tool rules with an identical argv dedupe into one entry.
//! - Aggressive items are unticked unless `--aggressive` is given, which also makes a
//!   rule's `cmd_aggressive` replace its `cmd`; items outside the roots are blocked.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{
    Bootout, Command, Delete, Deletes, Error, ItemKind, LaunchDomain, Package, Plan, Receipt,
    Report, RunAs, Selection, Size, Status, Tool, check_path,
};
use crate::fs::Argv;
use crate::process::IdlePath;

/// Whether `--aggressive` was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggressiveItems {
    Unticked,
    Ticked,
}

/// Which of a rule's twin fields matched (`docs/spec/rules.md#aggressive-twins`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Twin {
    Normal,
    Aggressive,
}

/// A tool rule's commands, as its `cmd` and `cmd_aggressive` fields give them, split
/// into argv by the rule parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCmds<'a> {
    Normal(&'a Argv),
    Aggressive(&'a Argv),
    Twins {
        cmd: &'a Argv,
        cmd_aggressive: &'a Argv,
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
    pub path: IdlePath,
    pub kind: ItemKind,
    pub size: Size,
    pub run_as: RunAs,
    pub reach: Reach,
    pub rule: String,
    pub twin: Twin,
}

pub struct PlanBuilder {
    aggressive: AggressiveItems,
    deletes: BTreeMap<PathBuf, Delete>,
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

    /// Adds a matched file or folder. A path matched again gains the rule; it is ticked
    /// when any of its matches is.
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

    /// Adds a tool rule's commands. Without `--aggressive` its `cmd` is ticked and its
    /// `cmd_aggressive` listed unticked; with it, `cmd_aggressive` replaces `cmd`. A
    /// command whose argv an earlier rule already added gains the rule instead of a
    /// second entry.
    pub fn add_tool(&mut self, rule: &str, cmds: ToolCmds<'_>) -> Result<(), Error> {
        let fields = match (cmds, self.aggressive) {
            (ToolCmds::Normal(cmd), _) => vec![(cmd, Twin::Normal)],
            (ToolCmds::Aggressive(cmd), _) => vec![(cmd, Twin::Aggressive)],
            (ToolCmds::Twins { cmd_aggressive, .. }, AggressiveItems::Ticked) => {
                vec![(cmd_aggressive, Twin::Aggressive)]
            }
            (
                ToolCmds::Twins {
                    cmd,
                    cmd_aggressive,
                },
                AggressiveItems::Unticked,
            ) => vec![(cmd, Twin::Normal), (cmd_aggressive, Twin::Aggressive)],
        };

        let commands = fields
            .into_iter()
            .map(|(argv, twin)| Command::from_argv(argv).map(|command| (command, twin)))
            .collect::<Result<Vec<_>, _>>()?;

        for (command, twin) in commands {
            self.add_command(rule, command, twin);
        }
        Ok(())
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

    pub fn build(self) -> Plan {
        Plan {
            deletes: Deletes::collapsing(self.deletes),
            tools: self.tools,
            receipts: self.receipts,
            reports: self.reports,
        }
    }

    // entries

    fn add_delete(&mut self, found: PathMatch) -> Result<&mut Delete, Error> {
        check_path(found.path.as_path())?;

        let status = self.path_status(found.reach, found.twin);
        let path = found.path.into_path();
        let entry = self.deletes.entry(path.clone()).or_insert(Delete {
            path,
            kind: found.kind,
            size: found.size,
            rules: Vec::new(),
            status,
            run_as: found.run_as,
            bootouts: Vec::new(),
        });

        entry.status = strongest(entry.status, status);
        if !entry.rules.contains(&found.rule) {
            entry.rules.push(found.rule);
            entry.rules.sort();
        }
        Ok(entry)
    }

    fn add_command(&mut self, rule: &str, command: Command, twin: Twin) {
        let selection = self.selection(twin);

        if let Some(tool) = self.tools.iter_mut().find(|tool| tool.command == command) {
            tool.selection = strongest_selection(tool.selection, selection);
            if !tool.rules.iter().any(|seen| seen == rule) {
                tool.rules.push(rule.to_owned());
            }
            return;
        }

        self.tools.push(Tool {
            rules: vec![rule.to_owned()],
            command,
            selection,
        });
    }

    // ticking

    fn path_status(&self, reach: Reach, twin: Twin) -> Status {
        match reach {
            Reach::OutsideRoots => Status::Blocked,
            Reach::InRoots => self.selection(twin).into(),
        }
    }

    fn selection(&self, twin: Twin) -> Selection {
        match (twin, self.aggressive) {
            (Twin::Normal, _) => Selection::Ticked,
            (Twin::Aggressive, AggressiveItems::Ticked) => Selection::Ticked,
            (Twin::Aggressive, AggressiveItems::Unticked) => Selection::Unticked,
        }
    }
}

/// The status of one path matched twice. Both matches share the path, so both are
/// blocked or neither is.
fn strongest(current: Status, added: Status) -> Status {
    match (current, added) {
        (Status::Blocked, _) | (_, Status::Blocked) => Status::Blocked,
        (Status::Ticked, _) | (_, Status::Ticked) => Status::Ticked,
        (Status::Unticked, Status::Unticked) => Status::Unticked,
    }
}

fn strongest_selection(current: Selection, added: Selection) -> Selection {
    match (current, added) {
        (Selection::Ticked, _) | (_, Selection::Ticked) => Selection::Ticked,
        (Selection::Unticked, Selection::Unticked) => Selection::Unticked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Runnable, argv};

    fn found(path: &str, rule: &str, twin: Twin) -> PathMatch {
        PathMatch {
            path: crate::plan::idle(path),
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
        builder.build()
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

    fn tool_summary(plan: &Plan) -> Vec<(Vec<String>, Vec<&str>, Selection)> {
        plan.tools()
            .iter()
            .map(|tool| {
                (
                    tool.rules.clone(),
                    tool.command.words().collect(),
                    tool.selection,
                )
            })
            .collect()
    }

    fn rules(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn keys_paths_and_lists_every_rule_that_matched() {
        let plan = built(
            AggressiveItems::Unticked,
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
            AggressiveItems::Unticked,
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

    #[test]
    fn outer_aggressive_match_hides_a_normal_one_inside() {
        let plan = built(
            AggressiveItems::Unticked,
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
        assert_eq!(plan.deletes()[0].status, Status::Unticked);
    }

    #[test]
    fn an_outer_entry_runs_elevated_when_a_collapsed_one_would() {
        let plan = built(
            AggressiveItems::Unticked,
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
    fn aggressive_items_are_unticked_unless_aggressive() {
        let matches = || {
            vec![
                found("/w/a", "rosie/a", Twin::Aggressive),
                found("/w/b", "rosie/b", Twin::Normal),
            ]
        };
        let statuses = |plan: Plan| -> Vec<Status> {
            plan.deletes().iter().map(|delete| delete.status).collect()
        };

        let default = built(AggressiveItems::Unticked, matches());
        let aggressive = built(AggressiveItems::Ticked, matches());

        assert_eq!(statuses(default), [Status::Unticked, Status::Ticked]);
        assert_eq!(statuses(aggressive), [Status::Ticked, Status::Ticked]);
    }

    #[test]
    fn a_path_matched_normally_and_aggressively_is_ticked() {
        let plan = built(
            AggressiveItems::Unticked,
            vec![
                found("/w/a", "rosie/strict", Twin::Aggressive),
                found("/w/a", "rosie/plain", Twin::Normal),
            ],
        );

        assert_eq!(plan.deletes()[0].status, Status::Ticked);
    }

    #[test]
    fn items_outside_roots_are_blocked_even_when_aggressive() {
        let outside = |twin| PathMatch {
            reach: Reach::OutsideRoots,
            ..found("/elsewhere/cache", "rosie/c", twin)
        };

        let plan = built(
            AggressiveItems::Ticked,
            vec![outside(Twin::Normal), outside(Twin::Aggressive)],
        );

        assert_eq!(plan.deletes()[0].status, Status::Blocked);
    }

    #[test]
    fn refuses_relative_paths() {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);

        let refused = builder.add_path(found("w/app/target", "rosie/cargo", Twin::Normal));

        assert!(matches!(refused, Err(Error::RelativePath { .. })));
    }

    /// `/w/app/../../etc` names `/etc`, and would collapse into `/w/app` as if inside it.
    #[test]
    fn refuses_dot_and_dot_dot_components() {
        let dotted = ["/w/app/../../etc", "/w/./app", "/w/app/.", "/w/.."];

        for path in dotted {
            let mut builder = PlanBuilder::new(AggressiveItems::Unticked);

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
            assert_eq!(builder.build(), Plan::default(), "{path}");
        }
    }

    /// `rosie run` must be able to read back every plan a scan writes, and the parser
    /// refuses an empty package or subject.
    #[test]
    fn refuses_an_empty_receipt_package_or_report_subject() {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);

        let package = builder.add_receipt("", Twin::Normal);
        let report = Report::new(String::new(), vec!["Remove X".to_owned()]);

        assert!(matches!(package, Err(Error::EmptyPackage)), "{package:?}");
        assert!(matches!(report, Err(Error::EmptySubject)), "{report:?}");
        assert_eq!(builder.build(), Plan::default());
    }

    /// A NUL byte ends a path or an argument at the operating system, so the item run
    /// would not be the item listed.
    #[test]
    fn refuses_a_nul_byte_in_any_path_or_argument() {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);

        let refusals = [
            builder.add_path(found("/w/a\0b", "rosie/cargo", Twin::Normal)),
            builder.add_launch_job(
                found("/w/x\0.plist", "rosie/launch", Twin::Normal),
                LaunchDomain::System,
            ),
            builder.add_tool("rosie/tool", ToolCmds::Normal(&argv("tool clean\0 --all"))),
            builder.add_tool(
                "rosie/tool",
                ToolCmds::Twins {
                    cmd: &argv("tool clean"),
                    cmd_aggressive: &argv("tool\0 clean --all"),
                },
            ),
            builder.add_receipt("com.x\0.pkg", Twin::Normal),
        ];

        for refused in refusals {
            assert!(matches!(refused, Err(Error::NulByte { .. })), "{refused:?}");
        }
        let plan = builder.build();
        assert_eq!(plan, Plan::default());
    }

    // launch jobs

    #[test]
    fn a_launch_job_boots_out_with_its_plists_status() {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);
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

        let plan = builder.build();

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
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);
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
        let plan = builder.build();

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
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);
        let path = "/L/LaunchAgents/com.x.plist";
        builder
            .add_launch_job(plist(path, Twin::Aggressive), LaunchDomain::Gui(501))
            .expect("valid plist");
        builder
            .add_launch_job(plist(path, Twin::Normal), LaunchDomain::Gui(501))
            .expect("valid plist");

        let plan = builder.build();

        assert_eq!(plan.deletes()[0].bootouts.len(), 1, "{plan:?}");
        assert_eq!(runnable_bootouts(&plan), [path]);
    }

    #[test]
    fn a_plist_ticked_by_a_plain_path_match_is_still_booted_out() {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);
        let path = "/L/LaunchAgents/com.x.plist";
        builder
            .add_launch_job(plist(path, Twin::Aggressive), LaunchDomain::Gui(501))
            .expect("valid plist");
        builder
            .add_path(plist(path, Twin::Normal))
            .expect("valid path");

        let plan = builder.build();

        assert_eq!(runnable_bootouts(&plan), [path]);
    }

    #[test]
    fn a_plist_inside_a_ticked_folder_is_booted_out_before_the_folder_goes() {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);
        let path = "/w/app/support/com.x.plist";
        builder
            .add_launch_job(plist(path, Twin::Aggressive), LaunchDomain::Gui(501))
            .expect("valid plist");
        builder
            .add_path(found("/w/app", "app/x", Twin::Normal))
            .expect("valid path");

        let plan = builder.build();

        assert_eq!(paths(&plan), ["/w/app"]);
        assert_eq!(runnable_bootouts(&plan), [path]);
    }

    #[test]
    fn refuses_a_plist_booted_out_of_two_domains() {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);
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

    #[test]
    fn tools_with_the_same_argv_dedupe_into_one_entry() {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);
        let prune = argv("docker system prune --force");
        let prune_all = argv("docker system prune --all --force");
        let cleanup = argv("brew cleanup");
        let adds = [
            (
                "rosie/docker",
                ToolCmds::Twins {
                    cmd: &prune,
                    cmd_aggressive: &prune_all,
                },
            ),
            ("user/docker", ToolCmds::Normal(&prune)),
            ("rosie/brew", ToolCmds::Normal(&cleanup)),
        ];
        for (rule, cmds) in adds {
            builder.add_tool(rule, cmds).expect("valid command");
        }

        let plan = builder.build();

        assert_eq!(
            tool_summary(&plan),
            [
                (
                    rules(&["rosie/docker", "user/docker"]),
                    vec!["docker", "system", "prune", "--force"],
                    Selection::Ticked,
                ),
                (
                    rules(&["rosie/docker"]),
                    vec!["docker", "system", "prune", "--all", "--force"],
                    Selection::Unticked,
                ),
                (
                    rules(&["rosie/brew"]),
                    vec!["brew", "cleanup"],
                    Selection::Ticked
                ),
            ]
        );
    }

    #[test]
    fn with_aggressive_cmd_aggressive_replaces_cmd() {
        let mut builder = PlanBuilder::new(AggressiveItems::Ticked);
        let twins = ToolCmds::Twins {
            cmd: &argv("docker system prune --force"),
            cmd_aggressive: &argv("docker system prune --all --force"),
        };
        builder.add_tool("rosie/docker", twins).expect("valid");
        builder
            .add_tool("rosie/brew", ToolCmds::Normal(&argv("brew cleanup")))
            .expect("valid");

        let plan = builder.build();

        assert_eq!(
            tool_summary(&plan),
            [
                (
                    rules(&["rosie/docker"]),
                    vec!["docker", "system", "prune", "--all", "--force"],
                    Selection::Ticked,
                ),
                (
                    rules(&["rosie/brew"]),
                    vec!["brew", "cleanup"],
                    Selection::Ticked
                ),
            ]
        );
    }
}
