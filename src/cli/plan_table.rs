//! The plan table: a plan for reading, in the `rosie check` columns plus a size column,
//! in the plan file's order (`docs/spec/plan.md#plan-table`).

use std::path::PathBuf;

use super::columns::{Align, Row, layout};
use super::entry_text::{sides, status_hints, sudo_note};
use super::shown::Shown;
use crate::hint::hints_text;
use crate::plan::{Delete, Plan, Receipt, Report, Status, Tool};

/// A tool or package forget has no size rosie can know.
const SIZE_UNKNOWN: &str = "?";

/// The table's lines: status, size, what the entry acts on, then its rules and notes.
pub fn plan_table(plan: &Plan, shown: &Shown) -> Vec<String> {
    let deletes = plan
        .deletes()
        .iter()
        .map(|delete| delete_row(delete, shown));
    let tools = plan.tools().iter().map(tool_row);
    let receipts = plan.receipts().iter().map(receipt_row);
    let reports = plan.reports().iter().map(report_row);
    let rows: Vec<Row<3>> = deletes
        .chain(tools)
        .chain(receipts)
        .chain(reports)
        .collect();

    layout([Align::Left, Align::Right, Align::Left], &rows)
}

/// The rules, `needs sudo`, the jobs it unloads, then the hints that would make it
/// clean; hints of several lines start on a line of their own.
fn delete_row(delete: &Delete, shown: &Shown) -> Row<3> {
    let path = shown.path(&delete.path);
    let rules = delete.rules.join(", ");
    let what = match &delete.status {
        Status::Clean | Status::OutsideRoots => rules,
        Status::Keep => format!("{rules} (aggressive)"),
        Status::Conflict(disagreement) => sides(disagreement),
    };
    let unloads = delete
        .bootouts
        .iter()
        .map(|bootout| format!("unloads {} job", bootout.domain));
    let notes: Vec<String> = std::iter::once(what)
        .chain(sudo_note(delete.run_as))
        .chain(unloads)
        .collect();

    let hints = status_hints(delete.status.kind(), &PathBuf::from(&path));
    let hints = hints_text(&hints);
    let text = match (hints.is_empty(), hints.contains('\n')) {
        (true, _) => notes.join("; "),
        (false, false) => format!("{}; {hints}", notes.join("; ")),
        (false, true) => format!("{}\n{hints}", notes.join("; ")),
    };
    Row {
        cells: [
            delete.status.label().to_owned(),
            delete.size.to_string(),
            path,
        ],
        text,
    }
}

fn tool_row(tool: &Tool) -> Row<3> {
    Row {
        cells: [
            Status::from(tool.selection).label().to_owned(),
            SIZE_UNKNOWN.to_owned(),
            format!("$ {}", tool.command.cmd()),
        ],
        text: tool.rule.clone(),
    }
}

fn receipt_row(receipt: &Receipt) -> Row<3> {
    Row {
        cells: [
            Status::from(receipt.selection).label().to_owned(),
            SIZE_UNKNOWN.to_owned(),
            format!("forget package {}", receipt.package.as_str()),
        ],
        text: String::new(),
    }
}

/// Report-only: no status of its own and nothing to size.
fn report_row(report: &Report) -> Row<3> {
    Row {
        cells: [
            "report".to_owned(),
            String::new(),
            report.subject().to_owned(),
        ],
        text: report.steps().join("; "),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const HOME: &str = "/Users/me";

    fn table(plan_toml: &str) -> Vec<String> {
        let plan = Plan::parse(&format!("version = 1\n{plan_toml}")).expect("valid plan");
        plan_table(
            &plan,
            &Shown {
                home: Path::new(HOME),
            },
        )
    }

    #[test]
    fn deletes_show_status_size_path_and_rules_with_their_notes() {
        let lines = table(
            r#"
[[delete]]
path = "/Users/me/src/app/node_modules"
type = "folder"
size = 1234567890
rules = ["rosie/node-modules"]
status = "clean"

[[delete]]
path = "/Users/me/src/old/.venv"
type = "folder"
size = 84000000
rules = ["rosie/python-venv"]
status = "keep"

[[delete]]
path = "/Users/me/x"
type = "file"
size = 10
rules = ["a/one", "b/two"]
status = "clean"
sudo = true
"#,
        );

        assert_eq!(
            lines,
            [
                "clean   1.2 GB  ~/src/app/node_modules  rosie/node-modules",
                "keep   84.0 MB  ~/src/old/.venv         rosie/python-venv (aggressive); to clean it: --aggressive",
                "clean     10 B  ~/x                     a/one, b/two; needs sudo",
            ]
            .map(str::to_owned)
        );
    }

    #[test]
    fn an_outside_roots_entry_lists_its_roots_hints_below_its_rules() {
        let lines = table(
            r#"
[[delete]]
path = "/Library/LaunchDaemons/com.x.plist"
type = "file"
size = 4096
rules = ["app/com.x"]
status = "outside-roots"
sudo = true

[[delete.bootout]]
plist = "/Library/LaunchDaemons/com.x.plist"
domain = "system"
"#,
        );

        let text_column =
            " ".repeat("outside-roots  4.1 KB  /Library/LaunchDaemons/com.x.plist  ".len());
        assert_eq!(
            lines,
            [[
                "outside-roots  4.1 KB  /Library/LaunchDaemons/com.x.plist  app/com.x; needs sudo; unloads system job".to_owned(),
                format!("{text_column}to allow just it:"),
                format!("{text_column}  rosie roots add /Library/LaunchDaemons/com.x.plist"),
                format!("{text_column}or to allow its folder:"),
                format!("{text_column}  rosie roots add /Library/LaunchDaemons"),
            ]
            .join("\n")]
        );
    }

    #[test]
    fn commands_and_reports_follow_the_deletes_in_plan_order() {
        let lines = table(
            r#"
[[delete]]
path = "/Users/me/a"
type = "file"
size = 2000
rules = ["rosie/a"]
status = "clean"

[[tool]]
rule = "user/homebrew"
shell = "/bin/zsh"
cmd = "brew cleanup"
status = "keep"

[tool.env]
HOMEBREW_NO_ENV_HINTS = "1"

[[forget]]
package = "com.x.pkg"
status = "clean"

[[report]]
subject = "login item X"
steps = ["Open System Settings > General > Login Items", "Remove X"]
"#,
        );

        assert_eq!(
            lines,
            [
                "clean   2.0 KB  ~/a                       rosie/a",
                "keep         ?  $ brew cleanup            user/homebrew",
                "clean        ?  forget package com.x.pkg",
                "report          login item X              Open System Settings > General > Login Items; Remove X",
            ]
            .map(str::to_owned)
        );
    }
}
