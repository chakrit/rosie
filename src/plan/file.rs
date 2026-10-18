//! The plan's TOML file: what `rosie scan` prints and `rosie run` reads back
//! (`docs/spec/plan.md#plan-file`).
//!
//! Each entry is written as its own table. Every entry but a report sits under a comment
//! with its size and, when it is `skip`, why, with the `rosie roots add` hint outside the
//! roots, or, when it
//! is a conflict, the rules on each side. A file is checked for its format version before
//! anything else in it is read, and unknown keys are refused: a plan never carries roots.
//! A `conflict` entry is refused too, so a plan read back always has every conflict
//! resolved (`docs/spec/plan.md#rule-conflicts`).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    Bootout, Delete, Deletes, EntryProblem, Error, ItemKind, LaunchDomain, Package, Plan, Receipt,
    Report, RunAs, Selection, Sides, Size, SkipReason, Status, StatusKind, Tool, check_path,
    comment, skip_note,
};
use crate::fs::ShellCommand;

const FORMAT_VERSION: u32 = 1;

impl Plan {
    /// The plan as TOML, for `rosie scan` to print.
    pub fn to_toml(&self) -> Result<String, Error> {
        let clean = StatusKind::Clean.label();
        let conflicts = match self.conflicts().len() {
            0 => String::new(),
            1 => format!("# 1 conflict: {ANSWER_IT}\n"),
            count => format!("# {count} conflicts: {ANSWER_IT}\n"),
        };
        let mut text = format!(
            "# rosie plan. Only entries with status = \"{clean}\" run.\n\
             # Delete any entry you do not want, then: rosie run <this file>\n\
             {conflicts}\
             version = {FORMAT_VERSION}\n"
        );

        // Conflicts first, where the user opening the file meets them.
        let (conflicted, decided): (Vec<&Delete>, Vec<&Delete>) = self
            .deletes
            .iter()
            .partition(|delete| matches!(delete.status, Status::Conflict(_)));
        for delete in conflicted.into_iter().chain(decided) {
            let note = match &delete.status {
                Status::Clean => delete.size.to_string(),
                Status::Skip(reason) => {
                    format!("{}; {}", delete.size, skip_note(*reason, &delete.path))
                }
                Status::Conflict(sides) => format!("{}; {sides}", delete.size),
            };
            push_entry(&mut text, Some(note), PlanFile::delete(delete))?;
        }
        for tool in &self.tools {
            let note = Some("size unknown".to_owned());
            push_entry(&mut text, note, PlanFile::tool(tool))?;
        }
        for receipt in &self.receipts {
            let note = Some("size unknown".to_owned());
            push_entry(&mut text, note, PlanFile::receipt(receipt))?;
        }
        for report in &self.reports {
            push_entry(&mut text, None, PlanFile::report(report))?;
        }
        Ok(text)
    }

    /// Reads a plan file back for `rosie run`.
    pub fn parse(text: &str) -> Result<Plan, Error> {
        let header: Header = toml::from_str(text)?;
        match header.version {
            None => return Err(Error::MissingVersion),
            Some(found) if found != i64::from(FORMAT_VERSION) => {
                let supported = FORMAT_VERSION;
                return Err(Error::UnsupportedVersion { found, supported });
            }
            Some(_) => {}
        }

        // Read from the text, not from a parsed table, so errors keep their line and column.
        let file: PlanFile = toml::from_str(text)?;
        file.into_plan(text)
    }
}

/// What a conflict needs, as the header's count says it.
const ANSWER_IT: &str = "set status to \"clean\" or \"skip\"";

/// The same, on a conflict's `status` line.
const ANSWER_HERE: &str = "set to \"clean\" or \"skip\"";

/// Appends one entry as its own table, under a `# note` comment, with a conflict's
/// `status` line saying what to set it to.
fn push_entry(text: &mut String, note: Option<String>, entry: PlanFile) -> Result<(), Error> {
    let conflict = format!("status = \"{}\"\n", StatusKind::Conflict.label());
    let answered = format!(
        "status = \"{}\"  # {ANSWER_HERE}\n",
        StatusKind::Conflict.label()
    );

    text.push('\n');
    if let Some(note) = note {
        text.push_str(&comment(&note));
    }
    let table = toml::to_string(&entry)?;
    text.push_str(&table.replacen(&conflict, &answered, 1));
    Ok(())
}

// the file's shape

/// Read first, so a plan of another format version is refused before its entries are.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Header {
    version: Option<i64>,
}

/// Loading only; [`PlanFile::into_plan`] validates.
#[derive(Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct PlanFile {
    /// Checked by [`Header`] first; declared so the key is not refused as unknown.
    #[serde(skip_serializing)]
    version: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    delete: Vec<DeleteTable>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool: Vec<ToolTable>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    forget: Vec<ForgetTable>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    report: Vec<ReportTable>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct DeleteTable {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    /// Absent is an error, not zero bytes.
    size: Option<u64>,
    rules: Vec<String>,
    status: StatusText,
    #[serde(skip_serializing_if = "is_false")]
    sudo: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bootout: Vec<BootoutTable>,
}

/// A delete's `status` and where it sits in the file being read; written plain.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct StatusText(toml::Spanned<String>);

impl StatusText {
    fn plain(label: &str) -> Self {
        StatusText(toml::Spanned::new(0..0, label.to_owned()))
    }

    /// The 1-based line of `text` the value starts on.
    fn line(&self, text: &str) -> usize {
        let before = &text[..self.0.span().start];
        before.matches('\n').count() + 1
    }
}

/// Absent reads as empty, which no status is.
impl Default for StatusText {
    fn default() -> Self {
        StatusText::plain("")
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct BootoutTable {
    plist: String,
    domain: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ToolTable {
    rule: String,
    shell: String,
    cmd: String,
    status: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    env: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ForgetTable {
    package: String,
    status: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ReportTable {
    subject: String,
    steps: Vec<String>,
}

fn is_false(value: &bool) -> bool {
    !value
}

// writing

impl PlanFile {
    fn delete(delete: &Delete) -> Self {
        let table = DeleteTable {
            path: path_text(&delete.path),
            kind: delete.kind.label().to_owned(),
            size: Some(delete.size.get()),
            rules: delete.rules.clone(),
            status: StatusText::plain(delete.status.label()),
            sudo: delete.run_as == RunAs::Sudo,
            bootout: delete.bootouts.iter().map(BootoutTable::of).collect(),
        };
        PlanFile {
            delete: vec![table],
            ..PlanFile::default()
        }
    }

    fn tool(tool: &Tool) -> Self {
        let table = ToolTable {
            rule: tool.rule.clone(),
            shell: path_text(tool.command.shell()),
            cmd: tool.command.cmd().to_owned(),
            status: tool.selection.label().to_owned(),
            env: tool.command.env().clone(),
        };
        PlanFile {
            tool: vec![table],
            ..PlanFile::default()
        }
    }

    fn receipt(receipt: &Receipt) -> Self {
        let table = ForgetTable {
            package: receipt.package.as_str().to_owned(),
            status: receipt.selection.label().to_owned(),
        };
        PlanFile {
            forget: vec![table],
            ..PlanFile::default()
        }
    }

    fn report(report: &Report) -> Self {
        let table = ReportTable {
            subject: report.subject().to_owned(),
            steps: report.steps().to_vec(),
        };
        PlanFile {
            report: vec![table],
            ..PlanFile::default()
        }
    }
}

impl BootoutTable {
    fn of(bootout: &Bootout) -> Self {
        BootoutTable {
            plist: path_text(&bootout.plist),
            domain: bootout.domain.to_string(),
        }
    }
}

/// Plan paths are UTF-8: the builder and the parser admit no others.
fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// reading

impl PlanFile {
    /// `text` is the file `self` was read from, for the line of each conflict.
    fn into_plan(self, text: &str) -> Result<Plan, Error> {
        let deletes = numbered("delete", self.delete, |table| table.into_delete(text))?;
        Ok(Plan {
            deletes: Deletes::refusing_nested(deletes)?,
            tools: numbered("tool", self.tool, ToolTable::into_tool)?,
            receipts: numbered("forget", self.forget, ForgetTable::into_receipt)?,
            reports: numbered("report", self.report, ReportTable::into_report)?,
        })
    }
}

/// Converts each table of a section, naming the section and the entry's number in the
/// file when one is invalid.
fn numbered<T, E>(
    section: &'static str,
    tables: Vec<T>,
    convert: impl Fn(T) -> Result<E, EntryProblem>,
) -> Result<Vec<E>, Error> {
    tables
        .into_iter()
        .enumerate()
        .map(|(index, table)| {
            convert(table).map_err(|problem| Error::InvalidEntry {
                section,
                number: index + 1,
                problem: Box::new(problem),
            })
        })
        .collect()
}

impl DeleteTable {
    fn into_delete(self, text: &str) -> Result<Delete, EntryProblem> {
        let path = absolute_path("path", self.path)?;
        let Some(kind) = ItemKind::parse(&self.kind) else {
            return Err(EntryProblem::UnknownKind { text: self.kind });
        };
        let Some(bytes) = self.size else {
            return Err(EntryProblem::MissingSize);
        };
        let status = status(&self.status, text)?;
        let run_as = match self.sudo {
            true => RunAs::Sudo,
            false => RunAs::User,
        };
        let bootouts = self
            .bootout
            .into_iter()
            .map(|table| table.into_bootout(&path))
            .collect::<Result<Vec<_>, _>>()?;
        refuse_repeated_plists(&bootouts)?;

        Ok(Delete {
            path,
            kind,
            size: Size::bytes(bytes),
            rules: self.rules,
            status,
            run_as,
            bootouts,
        })
    }
}

impl BootoutTable {
    /// A bootout belongs to the delete whose path is its plist or holds it, so it runs
    /// exactly when that delete does.
    fn into_bootout(self, delete_path: &Path) -> Result<Bootout, EntryProblem> {
        let plist = absolute_path("bootout plist", self.plist)?;
        if !plist.starts_with(delete_path) {
            return Err(EntryProblem::PlistOutsideDelete {
                plist,
                path: delete_path.to_path_buf(),
            });
        }
        let Some(domain) = LaunchDomain::parse(&self.domain) else {
            return Err(EntryProblem::UnknownDomain { text: self.domain });
        };
        Ok(Bootout { plist, domain })
    }
}

/// A delete carries one bootout per plist, whatever the domain, as the plan builder
/// writes it.
fn refuse_repeated_plists(bootouts: &[Bootout]) -> Result<(), EntryProblem> {
    let mut seen = HashSet::new();
    match bootouts.iter().find(|bootout| !seen.insert(&bootout.plist)) {
        Some(repeated) => Err(EntryProblem::RepeatedPlist {
            plist: repeated.plist.clone(),
        }),
        None => Ok(()),
    }
}

impl ToolTable {
    fn into_tool(self) -> Result<Tool, EntryProblem> {
        if self.shell.is_empty() {
            return Err(EntryProblem::MissingShell);
        }
        let command =
            ShellCommand::new(self.shell, self.cmd, self.env).map_err(EntryProblem::Command)?;
        let selection = selection(&self.status)?;
        Ok(Tool {
            rule: self.rule,
            command,
            selection,
        })
    }
}

impl ForgetTable {
    fn into_receipt(self) -> Result<Receipt, EntryProblem> {
        let package = Package::new(self.package).map_err(EntryProblem::Value)?;
        let selection = selection(&self.status)?;
        Ok(Receipt { package, selection })
    }
}

impl ReportTable {
    fn into_report(self) -> Result<Report, EntryProblem> {
        Report::new(self.subject, self.steps).map_err(EntryProblem::Value)
    }
}

fn absolute_path(field: &'static str, text: String) -> Result<PathBuf, EntryProblem> {
    let path = PathBuf::from(text);
    match check_path(&path) {
        Ok(()) => Ok(path),
        Err(source) => Err(EntryProblem::Path { field, source }),
    }
}

/// A `conflict` read back keeps its line in `file`, for `rosie run` to point to.
fn status(text: &StatusText, file: &str) -> Result<Status, EntryProblem> {
    let label: &String = text.0.as_ref();
    let Some(kind) = StatusKind::parse(label) else {
        return Err(EntryProblem::UnknownStatus {
            text: label.clone(),
        });
    };

    match kind {
        StatusKind::Clean => Ok(Status::Clean),
        StatusKind::Skip => Ok(Status::Skip(SkipReason::Chosen)),
        StatusKind::Conflict => Ok(Status::Conflict(Sides::Read {
            line: text.line(file),
        })),
    }
}

/// Commands are outside the roots gate and never conflict.
fn selection(text: &str) -> Result<Selection, EntryProblem> {
    match StatusKind::parse(text) {
        Some(StatusKind::Clean) => Ok(Selection::Clean),
        Some(StatusKind::Skip) => Ok(Selection::Skip),
        _ => Err(EntryProblem::UnknownSelection {
            text: text.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::plan::{AggressiveItems, PathMatch, PlanBuilder, Reach};
    use crate::tier::{Tier, Twin};

    fn found(path: &str, reach: Reach, tier: Tier) -> PathMatch {
        PathMatch {
            path: PathBuf::from(path),
            kind: ItemKind::Folder,
            size: Size::bytes(1_234_567),
            run_as: RunAs::User,
            reach,
            rule: "rosie/node".to_owned(),
            tier,
        }
    }

    /// A plan with one entry of every kind and status.
    fn every_kind() -> Plan {
        let mut builder = PlanBuilder::new(AggressiveItems::Skip);
        builder
            .add_path(found("/w/app/node_modules", Reach::InRoots, Tier::Normal))
            .expect("valid");
        builder
            .add_path(found("/w/app/.venv", Reach::InRoots, Tier::Aggressive))
            .expect("valid");
        builder
            .add_path(PathMatch {
                run_as: RunAs::Sudo,
                kind: ItemKind::File,
                ..found(
                    "/elsewhere/it's \"odd\"\n$x",
                    Reach::OutsideRoots,
                    Tier::Normal,
                )
            })
            .expect("valid");
        builder
            .add_launch_job(
                PathMatch {
                    kind: ItemKind::File,
                    run_as: RunAs::Sudo,
                    ..found(
                        "/Library/LaunchDaemons/com.x.helper.plist",
                        Reach::InRoots,
                        Tier::Normal,
                    )
                },
                LaunchDomain::System,
            )
            .expect("valid");
        builder
            .add_launch_job(
                found(
                    "/Users/me/Library/LaunchAgents/com.x.plist",
                    Reach::InRoots,
                    Tier::Normal,
                ),
                LaunchDomain::Gui(501),
            )
            .expect("valid");
        builder
            .add_launch_job(
                PathMatch {
                    kind: ItemKind::File,
                    ..found(
                        "/w/app/support/com.x.inner.plist",
                        Reach::InRoots,
                        Tier::Normal,
                    )
                },
                LaunchDomain::Gui(501),
            )
            .expect("valid");
        builder
            .add_path(found("/w/app/support", Reach::InRoots, Tier::Aggressive))
            .expect("valid");
        builder.add_tool(
            "rosie/docker",
            &Twin::Both {
                normal: crate::plan::tool_command("docker system prune --force"),
                aggressive: crate::plan::tool_command("docker system prune --all --force"),
            },
        );
        let brew = ShellCommand::new(
            "/bin/zsh".to_owned(),
            "brew cleanup && echo 'done'".to_owned(),
            BTreeMap::from([("HOMEBREW_NO_ENV_HINTS".to_owned(), "1".to_owned())]),
        )
        .expect("valid");
        builder.add_tool("user/brew", &Twin::Normal(brew));
        builder
            .add_receipt("com.x.pkg", Tier::Normal)
            .expect("valid");
        builder.add_report(
            Report::new(
                "login item X".to_owned(),
                vec![
                    "Open System Settings > General > Login Items".to_owned(),
                    "Remove X".to_owned(),
                ],
            )
            .expect("valid"),
        );
        builder.build().plan
    }

    /// `plan` as its file reads back: a skip's reason is only a comment there.
    fn as_read_back(plan: &Plan) -> Plan {
        let deletes = plan.deletes.restatusing(|delete| match &delete.status {
            Status::Skip(_) => Status::Skip(SkipReason::Chosen),
            other => other.clone(),
        });
        Plan {
            deletes,
            ..plan.clone()
        }
    }

    fn parse_error(text: &str) -> Error {
        Plan::parse(text).expect_err("plan should be refused")
    }

    #[test]
    fn round_trips_every_kind_of_entry() {
        let plan = every_kind();

        let text = plan.to_toml().expect("serializable");
        let parsed = Plan::parse(&text).expect("parses its own output");

        assert_eq!(parsed, as_read_back(&plan), "{text}");
        assert_eq!(parsed.deletes().len(), 6, "{text}");
        let bootouts: usize = parsed.deletes().iter().map(|d| d.bootouts.len()).sum();
        assert_eq!(bootouts, 3, "{text}");
    }

    #[test]
    fn a_bootout_is_written_inside_the_delete_that_holds_its_plist() {
        let text = every_kind().to_toml().expect("serializable");

        assert!(
            text.contains(
                "path = \"/w/app/support\"\ntype = \"folder\"\nsize = 1234567\n\
                 rules = [\"rosie/node\"]\nstatus = \"skip\"\n\n\
                 [[delete.bootout]]\nplist = \"/w/app/support/com.x.inner.plist\"\n\
                 domain = \"gui/501\"\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn refuses_a_bootout_outside_its_delete() {
        let text = "version = 1\n[[delete]]\npath = \"/w/a\"\ntype = \"folder\"\nsize = 1\n\
                    status = \"clean\"\n[[delete.bootout]]\nplist = \"/w/b/x.plist\"\n\
                    domain = \"system\"\n";

        let refused = parse_error(text);

        assert_eq!(
            refused.to_string(),
            "[[delete]] entry 1: bootout plist \"/w/b/x.plist\" is not inside path \"/w/a\""
        );
    }

    #[test]
    fn refuses_a_bootout_plist_listed_twice_in_any_domain() {
        for second_domain in ["system", "gui/501"] {
            let text = format!(
                "version = 1\n[[delete]]\npath = \"/w/a\"\ntype = \"folder\"\nsize = 1\n\
                 status = \"clean\"\n[[delete.bootout]]\nplist = \"/w/a/x.plist\"\n\
                 domain = \"system\"\n[[delete.bootout]]\nplist = \"/w/a/x.plist\"\n\
                 domain = \"{second_domain}\"\n"
            );

            let refused = parse_error(&text);

            assert_eq!(
                refused.to_string(),
                "[[delete]] entry 1: bootout plist \"/w/a/x.plist\" is listed twice",
                "{text}"
            );
        }
    }

    #[test]
    fn refuses_a_hand_written_plan_with_one_delete_inside_another() {
        let text = "version = 1\n\
                    [[delete]]\npath = \"/Users/me/Library/LaunchAgents\"\ntype = \"folder\"\n\
                    size = 1\nstatus = \"clean\"\n\n\
                    [[delete]]\npath = \"/Users/me/Library/LaunchAgents/com.x.plist\"\n\
                    type = \"file\"\nsize = 1\nstatus = \"clean\"\nsudo = true\n";

        let refused = parse_error(text);

        assert_eq!(
            refused.to_string(),
            "delete \"/Users/me/Library/LaunchAgents/com.x.plist\" is inside delete \
             \"/Users/me/Library/LaunchAgents\"; a scan never writes this, so run cannot \
             tell which order is safe"
        );
    }

    fn delete_table(path: &str) -> String {
        format!(
            "[[delete]]\npath = \"{path}\"\ntype = \"folder\"\nsize = 1\nstatus = \"clean\"\n\n"
        )
    }

    #[test]
    fn refuses_a_nested_pair_that_other_entries_separate_in_the_file() {
        let text = [
            "version = 1\n".to_owned(),
            delete_table("/w/app/node_modules/.cache"),
            delete_table("/w/other"),
            delete_table("/w/app/target"),
            delete_table("/w/app/node_modules"),
        ]
        .concat();

        let refused = parse_error(&text);

        assert!(
            matches!(&refused, Error::NestedDeletes { outer, inner }
                if outer == Path::new("/w/app/node_modules")
                    && inner == Path::new("/w/app/node_modules/.cache")),
            "{refused}"
        );
    }

    #[test]
    fn refuses_a_delete_listed_twice() {
        let text = [
            "version = 1\n".to_owned(),
            delete_table("/w/a"),
            delete_table("/w/a"),
        ]
        .concat();

        let refused = parse_error(&text);

        assert!(
            matches!(&refused, Error::RepeatedDelete { path } if path == Path::new("/w/a")),
            "{refused}"
        );
    }

    #[test]
    fn writes_the_format_version_and_readable_tables() {
        let text = every_kind().to_toml().expect("serializable");

        assert!(text.contains("\nversion = 1\n"), "{text}");
        assert!(
            text.contains("[[delete]]\npath = \"/w/app/node_modules\"\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "[[tool]]\nrule = \"rosie/docker\"\nshell = \"/bin/bash\"\n\
                 cmd = \"docker system prune --force\"\nstatus = \"clean\"\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("[tool.env]\nHOMEBREW_NO_ENV_HINTS = \"1\"\n"),
            "{text}"
        );
        assert!(text.contains("domain = \"system\""), "{text}");
        assert!(text.contains("domain = \"gui/501\""), "{text}");
    }

    #[test]
    fn comments_show_sizes_unknown_tool_sizes_and_the_roots_hint() {
        let text = every_kind().to_toml().expect("serializable");

        assert!(
            text.contains("# 1.2 MB\n[[delete]]\npath = \"/w/app/node_modules\""),
            "{text}"
        );
        assert!(text.contains("# size unknown\n[[tool]]"), "{text}");
        assert!(text.contains("# size unknown\n[[forget]]"), "{text}");
        assert!(
            text.contains(
                "# 1.2 MB; skip: outside the roots\n\
                 # to allow just it:\n\
                 #   rosie roots add '/elsewhere/it'\\''s \"odd\"'$'\\012''$x'\n\
                 # or to allow its folder:\n\
                 #   rosie roots add /elsewhere\n\
                 [[delete]]\n"
            ),
            "{text}"
        );
    }

    /// `/w/a` matched by two rules that disagree on its tier.
    fn conflicted_builder() -> PlanBuilder {
        let mut builder = PlanBuilder::new(AggressiveItems::Skip);
        for (rule, tier) in [("rosie/a", Tier::Normal), ("alice/b", Tier::Aggressive)] {
            let found = PathMatch {
                rule: rule.to_owned(),
                ..found("/w/a", Reach::InRoots, tier)
            };
            builder.add_path(found).expect("valid");
        }
        builder
    }

    fn conflicted() -> Plan {
        conflicted_builder().build().plan
    }

    #[test]
    fn a_conflict_is_written_first_under_a_count_with_what_to_set() {
        let mut builder = conflicted_builder();
        builder
            .add_path(found("/w/0", Reach::InRoots, Tier::Normal))
            .expect("valid");

        let text = builder.build().plan.to_toml().expect("serializable");

        assert!(
            text.contains("# 1 conflict: set status to \"clean\" or \"skip\"\nversion = 1\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "# 1.2 MB; rosie/a: normal, alice/b: aggressive\n[[delete]]\npath = \"/w/a\""
            ),
            "{text}"
        );
        assert!(
            text.contains("status = \"conflict\"  # set to \"clean\" or \"skip\"\n"),
            "{text}"
        );
        let conflict = text.find("path = \"/w/a\"").expect("the conflict");
        let decided = text.find("path = \"/w/0\"").expect("the clean entry");
        assert!(conflict < decided, "{text}");
    }

    #[test]
    fn a_conflict_read_back_keeps_the_line_of_its_status() {
        let text = conflicted().to_toml().expect("serializable");
        let line = 1 + text
            .lines()
            .position(|line| line.starts_with("status = \"conflict\""))
            .expect("a conflict status");

        let parsed = Plan::parse(&text).expect("a conflict reads back");

        assert_eq!(
            parsed.deletes()[0].status,
            Status::Conflict(Sides::Read { line })
        );
    }

    #[test]
    fn users_may_delete_entries() {
        let text = every_kind().to_toml().expect("serializable");
        let without_receipt = text.replace(
            "[[forget]]\npackage = \"com.x.pkg\"\nstatus = \"clean\"\n",
            "",
        );

        let parsed = Plan::parse(&without_receipt).expect("still a plan");

        assert!(parsed.receipts().is_empty(), "{without_receipt}");
        assert_eq!(parsed.deletes(), as_read_back(&every_kind()).deletes());
    }

    #[test]
    fn refuses_another_format_version_before_reading_entries() {
        let newer = "version = 2\n[[shred]]\npath = \"/w\"\n";

        let refused = parse_error(newer);

        assert!(
            matches!(
                refused,
                Error::UnsupportedVersion {
                    found: 2,
                    supported: 1
                }
            ),
            "{refused:?}"
        );
    }

    #[test]
    fn refuses_a_plan_without_a_version() {
        let refused = parse_error("[[forget]]\npackage = \"com.x\"\nstatus = \"clean\"\n");

        assert!(matches!(refused, Error::MissingVersion), "{refused:?}");
    }

    #[test]
    fn refuses_roots_in_a_plan() {
        let refused = parse_error("version = 1\nroots = [\"/\"]\n");

        assert!(matches!(refused, Error::Parse(_)), "{refused:?}");
    }

    /// Plans are edited by hand and can hold thousands of entries, so a key or value the
    /// format does not admit is reported with the line it is on.
    #[test]
    fn a_malformed_entry_is_reported_with_its_line() {
        let text = [
            "version = 1\n".to_owned(),
            delete_table("/w/a"),
            delete_table("/w/b"),
            "[[delete]]\npath = \"/w/c\"\ntype = \"folder\"\nsize = 1\nbogus = 3\n".to_owned(),
        ]
        .concat();
        let bogus_line = text
            .lines()
            .position(|line| line == "bogus = 3")
            .expect("present")
            + 1;

        let refused = parse_error(&text).to_string();

        assert!(refused.contains(&format!("line {bogus_line}")), "{refused}");
        assert!(refused.contains("bogus"), "{refused}");
    }

    #[test]
    fn refuses_invalid_entries_naming_them() {
        let cases = [
            (
                "version = 1\n[[delete]]\npath = \"/w/a\"\ntype = \"folder\"\nstatus = \"clean\"\n",
                "[[delete]] entry 1: size is missing",
            ),
            (
                "version = 1\n[[delete]]\npath = \"w/a\"\ntype = \"folder\"\nsize = 1\nstatus = \"clean\"\n",
                "[[delete]] entry 1: path \"w/a\" is not absolute",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/w/a\"\ntype = \"symlink\"\nsize = 1\nstatus = \"clean\"\n",
                "[[delete]] entry 1: type \"symlink\" is not one of file, folder",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/w/a\"\ntype = \"file\"\nsize = 1\n",
                "[[delete]] entry 1: status \"\" is not one of clean, skip, conflict",
            ),
            (
                "version = 1\n[[tool]]\nshell = \"/bin/bash\"\ncmd = \"a\"\nstatus = \"clean\"\n[[tool]]\nshell = \"/bin/bash\"\ncmd = \" \"\nstatus = \"clean\"\n",
                "[[tool]] entry 2: cmd is empty",
            ),
            (
                "version = 1\n[[tool]]\ncmd = \"a\"\nstatus = \"clean\"\n",
                "[[tool]] entry 1: shell is missing",
            ),
            (
                "version = 1\n[[tool]]\nshell = \"bash\"\ncmd = \"a\"\nstatus = \"clean\"\n",
                "[[tool]] entry 1: shell \"bash\" is not an absolute path",
            ),
            (
                "version = 1\n[[tool]]\nshell = \"/bin/bash\"\ncmd = \"a\"\nstatus = \"clean\"\n[tool.env]\n\"-i\" = \"1\"\n",
                "[[tool]] entry 1: env name \"-i\" is not letters, digits, and `_`, starting with no digit",
            ),
            (
                "version = 1\n[[tool]]\nshell = \"/bin/bash\"\ncmd = \"a\"\nstatus = \"conflict\"\n",
                "[[tool]] entry 1: status \"conflict\" is not one of clean, skip",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/L/x.plist\"\ntype = \"file\"\nsize = 1\nstatus = \"clean\"\n[[delete.bootout]]\nplist = \"/L/x.plist\"\ndomain = \"gui/me\"\n",
                "[[delete]] entry 1: bootout domain \"gui/me\" is not system or gui/<uid>",
            ),
            (
                "version = 1\n[[forget]]\nstatus = \"clean\"\n",
                "[[forget]] entry 1: package is missing",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/w/a\\u0000b\"\ntype = \"file\"\nsize = 1\nstatus = \"clean\"\n",
                "[[delete]] entry 1: path \"/w/a\\0b\" holds a NUL byte, which no path or argument can carry",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/L\"\ntype = \"folder\"\nsize = 1\nstatus = \"clean\"\n[[delete.bootout]]\nplist = \"/L/x\\u0000.plist\"\ndomain = \"system\"\n",
                "[[delete]] entry 1: bootout plist \"/L/x\\0.plist\" holds a NUL byte, which no path or argument can carry",
            ),
            (
                "version = 1\n[[tool]]\nshell = \"/bin/bash\"\ncmd = \"tool a\\u0000b\"\nstatus = \"clean\"\n",
                "[[tool]] entry 1: \"tool a\\0b\" holds a NUL byte, which no command can carry",
            ),
            (
                "version = 1\n[[forget]]\npackage = \"com.x\\u0000\"\nstatus = \"clean\"\n",
                "[[forget]] entry 1: \"com.x\\0\" holds a NUL byte, which no path or argument can carry",
            ),
            (
                "version = 1\n[[report]]\nsteps = [\"Remove X\"]\n",
                "[[report]] entry 1: subject is missing",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/w/app/../../etc\"\ntype = \"folder\"\nsize = 1\nstatus = \"clean\"\n",
                "[[delete]] entry 1: path \"/w/app/../../etc\" holds a . or .. component, which a plan path never holds",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/L\"\ntype = \"folder\"\nsize = 1\nstatus = \"clean\"\n[[delete.bootout]]\nplist = \"/L/./x.plist\"\ndomain = \"system\"\n",
                "[[delete]] entry 1: bootout plist \"/L/./x.plist\" holds a . or .. component, which a plan path never holds",
            ),
        ];

        for (text, message) in cases {
            assert_eq!(parse_error(text).to_string(), message, "{text}");
        }
    }
}
