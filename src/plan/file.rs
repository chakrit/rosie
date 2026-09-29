//! The plan's TOML file: what `rosie scan` prints and `rosie run` reads back
//! (`docs/spec/plan.md#plan-file`).
//!
//! Each entry is written as its own table. Every entry but a report sits under a comment
//! with its size and, when it is blocked, the `rosie roots add` hint. A file is checked
//! for its format version before anything else in it is read, and unknown keys are
//! refused: a plan never carries roots.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    Bootout, Command, Delete, Deletes, Error, ItemKind, LaunchDomain, Package, Plan, Receipt,
    Report, RunAs, Selection, Size, Status, Tool, blocked_hint, check_path,
};

pub const FORMAT_VERSION: u32 = 1;

const PREAMBLE: &str = "\
# rosie plan. Only entries with status = \"ticked\" run.
# Delete any entry you do not want, then: rosie run <this file>
";

impl Plan {
    /// The plan as TOML, for `rosie scan` to print.
    pub fn to_toml(&self) -> Result<String, Error> {
        let mut text = format!("{PREAMBLE}version = {FORMAT_VERSION}\n");

        for delete in self.deletes.iter() {
            let note = match delete.status {
                Status::Blocked => format!("{}; {}", delete.size, blocked_hint(&delete.path)),
                Status::Ticked | Status::Unticked => delete.size.to_string(),
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
        file.into_plan()
    }
}

/// Appends one entry as its own table, under a `# note` comment.
fn push_entry(text: &mut String, note: Option<String>, entry: PlanFile) -> Result<(), Error> {
    text.push('\n');
    if let Some(note) = note {
        text.push_str(&format!("# {note}\n"));
    }
    text.push_str(&toml::to_string(&entry)?);
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
    status: String,
    #[serde(skip_serializing_if = "is_false")]
    sudo: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bootout: Vec<BootoutTable>,
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
    rules: Vec<String>,
    cmd: Vec<String>,
    status: String,
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
            status: delete.status.label().to_owned(),
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
            rules: tool.rules.clone(),
            cmd: tool.command.words().map(str::to_owned).collect(),
            status: Status::from(tool.selection).label().to_owned(),
        };
        PlanFile {
            tool: vec![table],
            ..PlanFile::default()
        }
    }

    fn receipt(receipt: &Receipt) -> Self {
        let table = ForgetTable {
            package: receipt.package.as_str().to_owned(),
            status: Status::from(receipt.selection).label().to_owned(),
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
    fn into_plan(self) -> Result<Plan, Error> {
        let deletes = numbered("delete", self.delete, DeleteTable::into_delete)?;
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
    convert: impl Fn(T) -> Result<E, String>,
) -> Result<Vec<E>, Error> {
    tables
        .into_iter()
        .enumerate()
        .map(|(index, table)| {
            convert(table).map_err(|problem| Error::InvalidEntry {
                section,
                number: index + 1,
                problem,
            })
        })
        .collect()
}

impl DeleteTable {
    fn into_delete(self) -> Result<Delete, String> {
        let path = absolute_path("path", self.path)?;
        let kind = match self.kind.as_str() {
            "file" => ItemKind::File,
            "folder" => ItemKind::Folder,
            other => return Err(format!("type {other:?} is not one of file, folder")),
        };
        let Some(bytes) = self.size else {
            return Err("size is missing".to_owned());
        };
        let status = status(&self.status)?;
        let run_as = match self.sudo {
            true => RunAs::Sudo,
            false => RunAs::User,
        };
        let bootouts = self
            .bootout
            .into_iter()
            .map(|table| table.into_bootout(&path))
            .collect::<Result<_, _>>()?;

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
    fn into_bootout(self, delete_path: &Path) -> Result<Bootout, String> {
        let plist = absolute_path("bootout plist", self.plist)?;
        if !plist.starts_with(delete_path) {
            return Err(format!(
                "bootout plist {:?} is not inside path {:?}",
                plist.display().to_string(),
                delete_path.display().to_string()
            ));
        }
        let Some(domain) = LaunchDomain::parse(&self.domain) else {
            return Err(format!(
                "bootout domain {:?} is not system or gui/<uid>",
                self.domain
            ));
        };
        Ok(Bootout { plist, domain })
    }
}

impl ToolTable {
    fn into_tool(self) -> Result<Tool, String> {
        let command = Command::from_words(self.cmd).map_err(|error| error.to_string())?;
        let selection = selection(&self.status)?;
        Ok(Tool {
            rules: self.rules,
            command,
            selection,
        })
    }
}

impl ForgetTable {
    fn into_receipt(self) -> Result<Receipt, String> {
        let package = Package::new(self.package).map_err(|error| error.to_string())?;
        let selection = selection(&self.status)?;
        Ok(Receipt { package, selection })
    }
}

impl ReportTable {
    fn into_report(self) -> Result<Report, String> {
        Report::new(self.subject, self.steps).map_err(|error| error.to_string())
    }
}

fn absolute_path(field: &str, text: String) -> Result<PathBuf, String> {
    let path = PathBuf::from(text);
    match check_path(&path) {
        Ok(()) => Ok(path),
        Err(error) => Err(format!("{field} {error}")),
    }
}

fn status(text: &str) -> Result<Status, String> {
    match text {
        "ticked" => Ok(Status::Ticked),
        "unticked" => Ok(Status::Unticked),
        "blocked" => Ok(Status::Blocked),
        other => Err(format!(
            "status {other:?} is not one of ticked, unticked, blocked"
        )),
    }
}

/// Commands are outside the roots gate, so they are never blocked.
fn selection(text: &str) -> Result<Selection, String> {
    match text {
        "ticked" => Ok(Selection::Ticked),
        "unticked" => Ok(Selection::Unticked),
        other => Err(format!("status {other:?} is not one of ticked, unticked")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::plan::{AggressiveItems, PathMatch, PlanBuilder, Reach, ToolCmds, Twin};

    fn found(path: &str, reach: Reach, twin: Twin) -> PathMatch {
        PathMatch {
            path: PathBuf::from(path),
            kind: ItemKind::Folder,
            size: Size::bytes(1_234_567),
            run_as: RunAs::User,
            reach,
            rule: "rosie/node".to_owned(),
            twin,
        }
    }

    /// A plan with one entry of every kind and status.
    fn every_kind() -> Plan {
        let mut builder = PlanBuilder::new(AggressiveItems::Unticked);
        builder
            .add_path(found("/w/app/node_modules", Reach::InRoots, Twin::Normal))
            .expect("valid");
        builder
            .add_path(found("/w/app/.venv", Reach::InRoots, Twin::Aggressive))
            .expect("valid");
        builder
            .add_path(PathMatch {
                run_as: RunAs::Sudo,
                kind: ItemKind::File,
                ..found(
                    "/elsewhere/it's \"odd\"\n$x",
                    Reach::OutsideRoots,
                    Twin::Normal,
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
                        Twin::Normal,
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
                    Twin::Normal,
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
                        Twin::Normal,
                    )
                },
                LaunchDomain::Gui(501),
            )
            .expect("valid");
        builder
            .add_path(found("/w/app/support", Reach::InRoots, Twin::Aggressive))
            .expect("valid");
        builder
            .add_tool(
                "rosie/docker",
                ToolCmds::Twins {
                    cmd: "docker system prune --force",
                    cmd_aggressive: "docker system prune --all --force",
                },
            )
            .expect("valid");
        builder
            .add_tool(
                "user/docker",
                ToolCmds::Normal("docker system prune --force"),
            )
            .expect("valid");
        builder
            .add_receipt("com.x.pkg", Twin::Normal)
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
        builder.build()
    }

    fn parse_error(text: &str) -> Error {
        Plan::parse(text).expect_err("plan should be refused")
    }

    #[test]
    fn round_trips_every_kind_of_entry() {
        let plan = every_kind();

        let text = plan.to_toml().expect("serializable");
        let parsed = Plan::parse(&text).expect("parses its own output");

        assert_eq!(parsed, plan, "{text}");
        assert_eq!(parsed.deletes().len(), 6, "{text}");
        assert_eq!(parsed.runnable_as(RunAs::User).bootouts.len(), 2, "{text}");
    }

    #[test]
    fn a_bootout_is_written_inside_the_delete_that_holds_its_plist() {
        let text = every_kind().to_toml().expect("serializable");

        assert!(
            text.contains(
                "path = \"/w/app/support\"\ntype = \"folder\"\nsize = 1234567\n\
                 rules = [\"rosie/node\"]\nstatus = \"unticked\"\n\n\
                 [[delete.bootout]]\nplist = \"/w/app/support/com.x.inner.plist\"\n\
                 domain = \"gui/501\"\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn refuses_a_bootout_outside_its_delete() {
        let text = "version = 1\n[[delete]]\npath = \"/w/a\"\ntype = \"folder\"\nsize = 1\n\
                    status = \"ticked\"\n[[delete.bootout]]\nplist = \"/w/b/x.plist\"\n\
                    domain = \"system\"\n";

        let refused = parse_error(text);

        assert_eq!(
            refused.to_string(),
            "[[delete]] entry 1: bootout plist \"/w/b/x.plist\" is not inside path \"/w/a\""
        );
    }

    #[test]
    fn refuses_a_hand_written_plan_with_one_delete_inside_another() {
        let text = "version = 1\n\
                    [[delete]]\npath = \"/Users/me/Library/LaunchAgents\"\ntype = \"folder\"\n\
                    size = 1\nstatus = \"ticked\"\n\n\
                    [[delete]]\npath = \"/Users/me/Library/LaunchAgents/com.x.plist\"\n\
                    type = \"file\"\nsize = 1\nstatus = \"ticked\"\nsudo = true\n";

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
            "[[delete]]\npath = \"{path}\"\ntype = \"folder\"\nsize = 1\nstatus = \"ticked\"\n\n"
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
            text.contains("cmd = [\"docker\", \"system\", \"prune\", \"--force\"]"),
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
                "# 1.2 MB; blocked, outside roots. To allow it: \
                 rosie roots add '/elsewhere/it'\\''s \"odd\"'$'\\012''$x'\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn users_may_delete_entries() {
        let text = every_kind().to_toml().expect("serializable");
        let without_receipt = text.replace(
            "[[forget]]\npackage = \"com.x.pkg\"\nstatus = \"ticked\"\n",
            "",
        );

        let parsed = Plan::parse(&without_receipt).expect("still a plan");

        assert!(parsed.receipts().is_empty(), "{without_receipt}");
        assert_eq!(parsed.deletes(), every_kind().deletes());
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
        let refused = parse_error("[[forget]]\npackage = \"com.x\"\nstatus = \"ticked\"\n");

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
                "version = 1\n[[delete]]\npath = \"/w/a\"\ntype = \"folder\"\nstatus = \"ticked\"\n",
                "[[delete]] entry 1: size is missing",
            ),
            (
                "version = 1\n[[delete]]\npath = \"w/a\"\ntype = \"folder\"\nsize = 1\nstatus = \"ticked\"\n",
                "[[delete]] entry 1: path \"w/a\" is not absolute",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/w/a\"\ntype = \"symlink\"\nsize = 1\nstatus = \"ticked\"\n",
                "[[delete]] entry 1: type \"symlink\" is not one of file, folder",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/w/a\"\ntype = \"file\"\nsize = 1\n",
                "[[delete]] entry 1: status \"\" is not one of ticked, unticked, blocked",
            ),
            (
                "version = 1\n[[tool]]\ncmd = [\"a\"]\nstatus = \"ticked\"\n[[tool]]\ncmd = []\nstatus = \"ticked\"\n",
                "[[tool]] entry 2: cmd is empty",
            ),
            (
                "version = 1\n[[tool]]\ncmd = [\"a\"]\nstatus = \"blocked\"\n",
                "[[tool]] entry 1: status \"blocked\" is not one of ticked, unticked",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/L/x.plist\"\ntype = \"file\"\nsize = 1\nstatus = \"ticked\"\n[[delete.bootout]]\nplist = \"/L/x.plist\"\ndomain = \"gui/me\"\n",
                "[[delete]] entry 1: bootout domain \"gui/me\" is not system or gui/<uid>",
            ),
            (
                "version = 1\n[[forget]]\nstatus = \"ticked\"\n",
                "[[forget]] entry 1: package is missing",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/w/a\\u0000b\"\ntype = \"file\"\nsize = 1\nstatus = \"ticked\"\n",
                "[[delete]] entry 1: path \"/w/a\\0b\" holds a NUL byte, which no path or argument can carry",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/L\"\ntype = \"folder\"\nsize = 1\nstatus = \"ticked\"\n[[delete.bootout]]\nplist = \"/L/x\\u0000.plist\"\ndomain = \"system\"\n",
                "[[delete]] entry 1: bootout plist \"/L/x\\0.plist\" holds a NUL byte, which no path or argument can carry",
            ),
            (
                "version = 1\n[[tool]]\ncmd = [\"tool\", \"a\\u0000b\"]\nstatus = \"ticked\"\n",
                "[[tool]] entry 1: \"a\\0b\" holds a NUL byte, which no path or argument can carry",
            ),
            (
                "version = 1\n[[forget]]\npackage = \"com.x\\u0000\"\nstatus = \"ticked\"\n",
                "[[forget]] entry 1: \"com.x\\0\" holds a NUL byte, which no path or argument can carry",
            ),
            (
                "version = 1\n[[report]]\nsteps = [\"Remove X\"]\n",
                "[[report]] entry 1: subject is missing",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/w/app/../../etc\"\ntype = \"folder\"\nsize = 1\nstatus = \"ticked\"\n",
                "[[delete]] entry 1: path \"/w/app/../../etc\" holds a . or .. component, which a plan path never holds",
            ),
            (
                "version = 1\n[[delete]]\npath = \"/L\"\ntype = \"folder\"\nsize = 1\nstatus = \"ticked\"\n[[delete.bootout]]\nplist = \"/L/./x.plist\"\ndomain = \"system\"\n",
                "[[delete]] entry 1: bootout plist \"/L/./x.plist\" holds a . or .. component, which a plan path never holds",
            ),
        ];

        for (text, message) in cases {
            assert_eq!(parse_error(text).to_string(), message, "{text}");
        }
    }
}
