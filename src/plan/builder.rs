//! Builds a plan from what a scan found (`docs/spec/plan.md#plan-file`).
//!
//! - Path entries are keyed by path and list every rule that matched that path.
//! - Nested matches collapse into the outer one, which takes over their launch-job
//!   bootouts and runs elevated when any of them would.
//! - Tool rules with an identical trimmed `cmd` dedupe into one entry.
//! - Aggressive items are unticked unless `--aggressive` is given, which also makes a
//!   rule's `cmd_aggressive` replace its `cmd`; items outside the roots are blocked.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{
    Bootout, Command, Delete, Error, ItemKind, LaunchDomain, Package, Plan, Receipt, Report, RunAs,
    Selection, Size, Status, Tool, check_path,
};

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

/// A tool rule's commands, as its `cmd` and `cmd_aggressive` fields give them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCmds<'a> {
    Normal(&'a str),
    Aggressive(&'a str),
    Twins {
        cmd: &'a str,
        cmd_aggressive: &'a str,
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

pub struct PlanBuilder {
    aggressive: AggressiveItems,
    deletes: BTreeMap<PathBuf, Delete>,
    /// Keyed by the trimmed `cmd` text.
    tools: Vec<(String, Tool)>,
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
            .get(&plist.path)
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
    /// command whose trimmed text an earlier rule already added gains the rule instead of
    /// a second entry.
    pub fn add_tool(&mut self, rule: &str, cmds: ToolCmds<'_>) -> Result<(), Error> {
        let fields = match cmds {
            ToolCmds::Normal(cmd) => vec![(cmd, Twin::Normal)],
            ToolCmds::Aggressive(cmd) => vec![(cmd, Twin::Aggressive)],
            ToolCmds::Twins {
                cmd,
                cmd_aggressive,
            } => vec![(cmd, Twin::Normal), (cmd_aggressive, Twin::Aggressive)],
        };
        let replaced = match (cmds, self.aggressive) {
            (ToolCmds::Twins { .. }, AggressiveItems::Ticked) => Some(Twin::Normal),
            _ => None,
        };

        // Every field is checked, including one `--aggressive` replaces, so a broken rule
        // fails the same way with or without the flag.
        let commands = fields
            .into_iter()
            .map(|(cmd, twin)| Command::split(rule, cmd).map(|command| (cmd.trim(), command, twin)))
            .collect::<Result<Vec<_>, _>>()?;

        let listed = commands
            .into_iter()
            .filter(|(_, _, twin)| Some(*twin) != replaced);
        for (key, command, twin) in listed {
            self.add_command(rule, key, command, twin);
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
            deletes: collapse_nested(self.deletes),
            tools: self.tools.into_iter().map(|(_, tool)| tool).collect(),
            receipts: self.receipts,
            reports: self.reports,
        }
    }

    // entries

    fn add_delete(&mut self, found: PathMatch) -> Result<&mut Delete, Error> {
        check_path(&found.path)?;

        let status = self.path_status(found.reach, found.twin);
        let entry = self.deletes.entry(found.path.clone()).or_insert(Delete {
            path: found.path,
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

    /// `key` is the trimmed `cmd` text that tools dedupe on.
    fn add_command(&mut self, rule: &str, key: &str, command: Command, twin: Twin) {
        let selection = self.selection(twin);

        if let Some((_, tool)) = self.tools.iter_mut().find(|(seen, _)| seen == key) {
            tool.selection = strongest_selection(tool.selection, selection);
            if !tool.rules.iter().any(|seen| seen == rule) {
                tool.rules.push(rule.to_owned());
            }
            return;
        }

        let tool = Tool {
            rules: vec![rule.to_owned()],
            command,
            selection,
        };
        self.tools.push((key.to_owned(), tool));
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

/// Keeps only the outermost of nested paths. `Path` orders by component, so every path
/// inside another sorts directly after it.
///
/// The outer entry keeps its own rules, size, and status. It takes over the bootouts of
/// the entries inside it, so their jobs are unloaded exactly when it is deleted, and runs
/// elevated when any of them would.
fn collapse_nested(deletes: BTreeMap<PathBuf, Delete>) -> Vec<Delete> {
    let mut outermost: Vec<Delete> = Vec::with_capacity(deletes.len());
    for delete in deletes.into_values() {
        match outermost.last_mut() {
            Some(outer) if delete.path.starts_with(&outer.path) => absorb(outer, delete),
            _ => outermost.push(delete),
        }
    }
    outermost
}

fn absorb(outer: &mut Delete, inner: Delete) {
    outer.bootouts.extend(inner.bootouts);
    if inner.run_as == RunAs::Sudo {
        outer.run_as = RunAs::Sudo;
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
        builder.build()
    }

    fn paths(plan: &Plan) -> Vec<&str> {
        plan.deletes()
            .iter()
            .map(|delete| delete.path.to_str().expect("utf-8"))
            .collect()
    }

    /// The plists a run boots out, in order.
    fn runnable_bootouts(plan: &Plan) -> Vec<&str> {
        plan.runnable_as(RunAs::User)
            .bootouts
            .iter()
            .map(|bootout| bootout.plist.to_str().expect("utf-8"))
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
            builder.add_tool("rosie/tool", ToolCmds::Normal("tool clean\0 --all")),
            builder.add_tool(
                "rosie/tool",
                ToolCmds::Twins {
                    cmd: "tool clean",
                    cmd_aggressive: "tool\0 clean --all",
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
    fn tools_with_the_same_trimmed_cmd_dedupe_into_one_entry() {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);
        let adds = [
            (
                "rosie/docker",
                ToolCmds::Twins {
                    cmd: "docker system prune --force",
                    cmd_aggressive: "docker system prune --all --force",
                },
            ),
            (
                "user/docker",
                ToolCmds::Normal("  docker system prune --force\n"),
            ),
            ("rosie/brew", ToolCmds::Normal("brew cleanup")),
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
            cmd: "docker system prune --force",
            cmd_aggressive: "docker system prune --all --force",
        };
        builder.add_tool("rosie/docker", twins).expect("valid");
        builder
            .add_tool("rosie/brew", ToolCmds::Normal("brew cleanup"))
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

    #[test]
    fn refuses_an_empty_tool_command() {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);

        let refused = builder.add_tool("rosie/blank", ToolCmds::Normal("   "));

        assert!(
            matches!(refused, Err(Error::EmptyCommand { ref rule }) if rule == "rosie/blank"),
            "{refused:?}"
        );
    }

    #[test]
    fn refuses_a_tool_command_with_quotes_or_backslashes_naming_the_rule() {
        let quoted = [
            r#"find /tmp -name "a b" -delete"#,
            "find /tmp -name 'a b' -delete",
            r"find /tmp -name a\ b -delete",
        ];

        for cmd in quoted {
            let mut builder = PlanBuilder::new(AggressiveItems::Ticked);
            let twins = ToolCmds::Twins {
                cmd,
                cmd_aggressive: "tool clean --all",
            };

            let refused = builder.add_tool("user/find", twins);

            assert!(
                matches!(refused, Err(Error::QuotedCommand { ref rule, .. }) if rule == "user/find"),
                "{cmd}: {refused:?}"
            );
        }
    }
}
