//! Trimming a plan in the `clean` checklist (`docs/spec/plan.md#interactive-confirmation`).
//!
//! The checklist lists the plan's `clean` entries, all checked. An entry the user
//! unchecks is removed from the plan, as if deleted from a plan file before `rosie run`;
//! nothing can be added or made `clean` (`docs/spec/plan.md#editing-a-plan`).

use std::collections::BTreeSet;
use std::fmt;
use std::ptr;

use super::{Delete, Plan, Receipt, Tool};

/// A `clean` entry, as the checklist lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanEntry<'a> {
    Delete(&'a Delete),
    Tool(&'a Tool),
    Receipt(&'a Receipt),
}

/// `delete /path (1.2 GB)`, `run docker system prune --force`, `forget com.x.pkg`.
impl fmt::Display for CleanEntry<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CleanEntry::Delete(delete) => {
                write!(f, "delete {} ({})", delete.path.display(), delete.size)
            }
            CleanEntry::Tool(tool) => write!(f, "run {}", tool.command),
            CleanEntry::Receipt(receipt) => write!(f, "forget {}", receipt.package.as_str()),
        }
    }
}

impl Plan {
    /// The `clean` entries: deletes, then tools, then receipts, each in plan order.
    pub fn clean_entries(&self) -> Vec<CleanEntry<'_>> {
        let deletes = self
            .deletes
            .iter()
            .filter(|delete| delete.status.is_clean())
            .map(CleanEntry::Delete);
        let tools = self
            .tools
            .iter()
            .filter(|tool| tool.selection.is_clean())
            .map(CleanEntry::Tool);
        let receipts = self
            .receipts
            .iter()
            .filter(|receipt| receipt.selection.is_clean())
            .map(CleanEntry::Receipt);

        deletes.chain(tools).chain(receipts).collect()
    }

    /// The plan without the `clean` entries missing from `kept`, entries taken from
    /// [`Plan::clean_entries`] and told apart by identity, not by value. An entry of
    /// another plan keeps nothing. `keep`, `outside-roots`, and report entries stay as
    /// they are.
    pub fn keeping_clean_entries(&self, kept: &[CleanEntry<'_>]) -> Plan {
        let kept = Kept::from(kept);

        let deletes = self.deletes.retaining(|delete| kept.keeps_delete(delete));
        let tools = self
            .tools
            .iter()
            .filter(|tool| kept.keeps_tool(tool))
            .cloned()
            .collect();
        let receipts = self
            .receipts
            .iter()
            .filter(|receipt| kept.keeps_receipt(receipt))
            .cloned()
            .collect();

        Plan {
            deletes,
            tools,
            receipts,
            reports: self.reports.clone(),
        }
    }
}

/// The addresses of the kept entries: an entry's identity within the plan it borrows.
#[derive(Default)]
struct Kept {
    deletes: BTreeSet<*const Delete>,
    tools: BTreeSet<*const Tool>,
    receipts: BTreeSet<*const Receipt>,
}

impl From<&[CleanEntry<'_>]> for Kept {
    fn from(entries: &[CleanEntry<'_>]) -> Self {
        let mut kept = Kept::default();
        for entry in entries {
            match *entry {
                CleanEntry::Delete(delete) => kept.deletes.insert(ptr::from_ref(delete)),
                CleanEntry::Tool(tool) => kept.tools.insert(ptr::from_ref(tool)),
                CleanEntry::Receipt(receipt) => kept.receipts.insert(ptr::from_ref(receipt)),
            };
        }
        kept
    }
}

/// Whether an entry stays in the trimmed plan: every entry the checklist did not list,
/// and each listed one the user left checked.
impl Kept {
    fn keeps_delete(&self, delete: &Delete) -> bool {
        !delete.status.is_clean() || self.deletes.contains(&ptr::from_ref(delete))
    }

    fn keeps_tool(&self, tool: &Tool) -> bool {
        !tool.selection.is_clean() || self.tools.contains(&ptr::from_ref(tool))
    }

    fn keeps_receipt(&self, receipt: &Receipt) -> bool {
        !receipt.selection.is_clean() || self.receipts.contains(&ptr::from_ref(receipt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = r#"version = 1

[[delete]]
path = "/Users/me/a/node_modules"
type = "folder"
size = 4096
rules = ["rosie/node"]
status = "clean"

[[delete]]
path = "/Users/me/b/node_modules"
type = "folder"
size = 8192
rules = ["rosie/node"]
status = "keep"

[[delete]]
path = "/Library/Caches/x"
type = "folder"
size = 4096
rules = ["rosie/x"]
status = "outside-roots"

[[delete]]
path = "/Users/me/c/target"
type = "folder"
size = 4096
rules = ["rosie/cargo"]
status = "clean"

[[tool]]
rule = "rosie/docker"
shell = "/bin/bash"
cmd = "docker system prune --force"
status = "clean"

[[report]]
subject = "login item X"
steps = ["Remove X"]
"#;

    fn plan() -> Plan {
        Plan::parse(PLAN).expect("valid plan")
    }

    fn labels(plan: &Plan) -> Vec<String> {
        plan.clean_entries()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn lists_only_clean_entries() {
        assert_eq!(
            labels(&plan()),
            [
                "delete /Users/me/a/node_modules (4.1 KB)",
                "delete /Users/me/c/target (4.1 KB)",
                "run docker system prune --force",
            ]
        );
    }

    #[test]
    fn unchecked_checklist_entries_are_removed_and_the_rest_is_kept() {
        let plan = plan();
        let kept = [plan.clean_entries()[1]];

        let trimmed = plan.keeping_clean_entries(&kept);

        assert_eq!(labels(&trimmed), ["delete /Users/me/c/target (4.1 KB)"]);
        let paths: Vec<_> = trimmed
            .deletes()
            .iter()
            .map(|delete| delete.path.to_str().expect("utf-8"))
            .collect();
        assert_eq!(
            paths,
            [
                "/Library/Caches/x",
                "/Users/me/b/node_modules",
                "/Users/me/c/target"
            ]
        );
        assert!(trimmed.tools().is_empty());
        assert_eq!(trimmed.reports(), plan.reports());
    }

    #[test]
    fn an_equal_entry_of_another_plan_keeps_nothing() {
        let shown = plan();
        let other = plan();
        let kept = other.clean_entries();

        let trimmed = shown.keeping_clean_entries(&kept);

        assert!(trimmed.clean_entries().is_empty());
    }
}
