//! The plan listing: a plan for reading (`docs/spec/plan.md#plan-listing`). What the
//! scan found sits under the folder holding it, a blank line between folders; each item
//! is a row of space saved, rule, and name, with a further rule on each line below it,
//! then a note on why it is not cleaned. The commands follow, then the steps by hand.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::args::Mode;
use super::console::Width;
use super::entry_text::{aggressive_hint, status_hints, sudo_note};
use super::findings::walk_skip_hints;
use super::shown::Shown;
use crate::hint::{Hint, hint_lines};
use crate::output::{Palette, Tone};
use crate::plan::{Delete, Plan, Receipt, Report, Selection, Size, SkipReason, Status, Tool};
use crate::scan::{Refused, Skipped};

/// What the scan left out of the plan, which the listing shows beside it.
pub struct LeftOut<'s> {
    pub skipped: &'s [Skipped],
    pub refused: &'s [Refused],
    /// Decides which flags a skip may point to.
    pub mode: &'s Mode,
}

/// Sizes from here up are shown strong.
const LARGE: Size = Size::bytes(1_000_000_000);

/// Rows start here, under their heading.
const INDENT: &str = "  ";

/// A name trimmed to fit keeps at least this many characters.
const MIN_NAME: usize = 12;

/// The listing's lines, each fitting `width` when it is bounded.
pub fn plan_listing(
    plan: &Plan,
    left_out: &LeftOut,
    shown: &Shown,
    palette: Palette,
    width: Width,
) -> Vec<String> {
    let sections = sections(plan, left_out, shown);
    let columns = Columns::of(&sections);

    let blocks: Vec<Vec<String>> = sections
        .iter()
        .map(|section| section.lines(&columns, palette, width))
        .collect();
    blocks.join(&String::new())
}

// the model

/// A heading and the items under it.
struct Section {
    heading: Heading,
    items: Vec<Item>,
}

enum Heading {
    /// A folder's shown path; its items' names are paths, trimmed to fit.
    Folder(String),
    Commands,
    ByHand,
}

/// One item before it is laid out.
struct Item {
    size: SizeCell,
    /// Whether running the plan frees the size; a size not freed is shown quiet.
    saved: bool,
    rules: Vec<String>,
    /// The last column.
    name: String,
    note: Option<Note>,
    /// Lines below the note: a report's steps.
    steps: Vec<String>,
}

enum SizeCell {
    Known(Size),
    /// A tool command or package forget: rosie cannot know it.
    Unknown,
    Blank,
}

impl SizeCell {
    fn text(&self) -> String {
        match self {
            SizeCell::Known(size) => size.to_string(),
            SizeCell::Unknown => "?".to_owned(),
            SizeCell::Blank => String::new(),
        }
    }
}

/// Why an item is not cleaned, or what cleaning it takes: status words in their tone,
/// then details, then the hints that would change it.
struct Note {
    status: Option<(Tone, String)>,
    details: Vec<String>,
    hints: Vec<Hint>,
}

impl Note {
    fn is_empty(&self) -> bool {
        self.status.is_none() && self.details.is_empty() && self.hints.is_empty()
    }
}

// building the model

/// Folders in path order, items by name within each, then the commands, then the steps
/// by hand.
fn sections(plan: &Plan, left_out: &LeftOut, shown: &Shown) -> Vec<Section> {
    let deletes = plan
        .deletes()
        .iter()
        .map(|delete| (delete.path.as_path(), delete_item(delete, shown)));
    let refused = left_out
        .refused
        .iter()
        .map(|refused| (refused.path.as_path(), refused_item(refused)));
    let skipped = left_out.skipped.iter().map(|skipped| {
        (
            skipped.path.as_path(),
            skipped_item(skipped, shown, left_out.mode),
        )
    });

    let mut folders: BTreeMap<PathBuf, Vec<Item>> = BTreeMap::new();
    for (path, item) in deletes.chain(refused).chain(skipped) {
        let folder = path.parent().unwrap_or(path);
        folders.entry(folder.to_path_buf()).or_default().push(item);
    }
    let folders = folders.into_iter().map(|(folder, mut items)| {
        items.sort_by(|a, b| a.name.cmp(&b.name));
        Section {
            heading: Heading::Folder(shown.path(&folder)),
            items,
        }
    });

    let tools = plan.tools().iter().map(tool_item);
    let forgets = plan.receipts().iter().map(forget_item);
    let commands = Section {
        heading: Heading::Commands,
        items: tools.chain(forgets).collect(),
    };
    let by_hand = Section {
        heading: Heading::ByHand,
        items: plan.reports().iter().map(report_item).collect(),
    };

    folders
        .chain([commands, by_hand])
        .filter(|section| !section.items.is_empty())
        .collect()
}

fn delete_item(delete: &Delete, shown: &Shown) -> Item {
    let rules = match &delete.status {
        Status::Conflict(disagreement) => {
            let normal = disagreement
                .normal()
                .iter()
                .map(|rule| format!("{rule} (normal)"));
            let aggressive = disagreement
                .aggressive()
                .iter()
                .map(|rule| format!("{rule} (aggressive)"));
            normal.chain(aggressive).collect()
        }
        Status::Clean | Status::Skip(_) => delete.rules.clone(),
    };
    let status = match &delete.status {
        Status::Clean => None,
        Status::Skip(SkipReason::Aggressive) => Some((Tone::Calm, "skip (aggressive)".to_owned())),
        Status::Skip(SkipReason::OutsideRoots) => {
            Some((Tone::Alarm, "skip (outside the roots)".to_owned()))
        }
        Status::Skip(SkipReason::Chosen) => Some((Tone::Calm, "skip".to_owned())),
        Status::Conflict(_) => Some((Tone::Caution, "conflict".to_owned())),
    };
    let unloads = delete
        .bootouts
        .iter()
        .map(|bootout| format!("unloads {} job", bootout.domain));
    let note = Note {
        status,
        details: sudo_note(delete.run_as)
            .into_iter()
            .chain(unloads)
            .collect(),
        hints: status_hints(&delete.status, &PathBuf::from(shown.path(&delete.path))),
    };

    Item {
        size: SizeCell::Known(delete.size),
        saved: delete.status.is_clean(),
        rules,
        name: name(&delete.path),
        note: Some(note),
        steps: Vec::new(),
    }
}

fn refused_item(refused: &Refused) -> Item {
    let program = refused
        .program
        .file_name()
        .unwrap_or(refused.program.as_os_str());
    let status = format!(
        "in use: {} (pid {})",
        program.to_string_lossy(),
        refused.pid
    );
    Item {
        size: SizeCell::Known(refused.size),
        saved: false,
        rules: refused.rules.clone(),
        name: name(&refused.path),
        note: Some(Note {
            status: Some((Tone::Caution, status)),
            details: Vec::new(),
            hints: Vec::new(),
        }),
        steps: Vec::new(),
    }
}

fn skipped_item(skipped: &Skipped, shown: &Shown, mode: &Mode) -> Item {
    let status = format!("skipped: {}", skipped.reason.reason());
    let hints = walk_skip_hints(skipped, Path::new(&shown.path(&skipped.path)), mode);
    Item {
        size: SizeCell::Blank,
        saved: false,
        rules: Vec::new(),
        name: name(&skipped.path),
        note: Some(Note {
            status: Some((Tone::Calm, status)),
            details: Vec::new(),
            hints,
        }),
        steps: Vec::new(),
    }
}

fn tool_item(tool: &Tool) -> Item {
    Item {
        size: SizeCell::Unknown,
        saved: tool.selection == Selection::Clean,
        rules: vec![tool.rule.clone()],
        name: format!("$ {}", tool.command.cmd()),
        note: Some(selection_note(tool.selection)),
        steps: Vec::new(),
    }
}

fn forget_item(receipt: &Receipt) -> Item {
    Item {
        size: SizeCell::Unknown,
        saved: receipt.selection == Selection::Clean,
        rules: Vec::new(),
        name: format!("forget package {}", receipt.package.as_str()),
        note: Some(selection_note(receipt.selection)),
        steps: Vec::new(),
    }
}

/// A tool command or package forget is `clean` or `skip`; a scan skips one only as
/// aggressive.
fn selection_note(selection: Selection) -> Note {
    let (status, hints) = match selection {
        Selection::Clean => (None, Vec::new()),
        Selection::Skip => (
            Some((Tone::Calm, "skip (aggressive)".to_owned())),
            vec![aggressive_hint()],
        ),
    };
    Note {
        status,
        details: Vec::new(),
        hints,
    }
}

fn report_item(report: &Report) -> Item {
    Item {
        size: SizeCell::Blank,
        saved: false,
        rules: Vec::new(),
        name: report.subject().to_owned(),
        note: None,
        steps: report.steps().to_vec(),
    }
}

/// The last part of `path`; the whole path when it has none.
fn name(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.display().to_string(),
    }
}

// laying it out

/// The size and rule columns' widths, the same for every section.
struct Columns {
    size: usize,
    rule: usize,
}

impl Columns {
    fn of(sections: &[Section]) -> Columns {
        let items = || sections.iter().flat_map(|section| &section.items);
        let size = items().map(|item| width_of(&item.size.text())).max();
        let rule = items()
            .flat_map(|item| &item.rules)
            .map(|rule| width_of(rule))
            .max();
        Columns {
            size: size.unwrap_or(0),
            rule: rule.unwrap_or(0),
        }
    }

    /// Where notes and further rules start: the rule column.
    fn under_rule(&self) -> String {
        format!("{INDENT}{}  ", " ".repeat(self.size))
    }

    /// How much of `width` the name column may take.
    fn name_room(&self, width: Width) -> Option<usize> {
        let before_name = INDENT.len() + self.size + 2 + self.rule + 2;
        match width {
            Width::Unbounded => None,
            Width::Columns(columns) => Some(columns.saturating_sub(before_name).max(MIN_NAME)),
        }
    }
}

impl Section {
    fn lines(&self, columns: &Columns, palette: Palette, width: Width) -> Vec<String> {
        let (heading, name_room) = match &self.heading {
            Heading::Folder(path) => {
                let room = match width {
                    Width::Unbounded => None,
                    Width::Columns(columns) => Some(columns.max(MIN_NAME)),
                };
                (palette.folder(&fit(path, room)), columns.name_room(width))
            }
            Heading::Commands => ("commands".to_owned(), None),
            Heading::ByHand => ("by hand".to_owned(), None),
        };

        let items = self
            .items
            .iter()
            .flat_map(|item| item.lines(columns, name_room, palette));
        std::iter::once(heading).chain(items).collect()
    }
}

impl Item {
    /// The row, a line per further rule, the note, then the steps.
    fn lines(&self, columns: &Columns, name_room: Option<usize>, palette: Palette) -> Vec<String> {
        let under_rule = columns.under_rule();

        let size = format!("{:>width$}", self.size.text(), width = columns.size);
        let size = match (&self.size, self.saved) {
            (SizeCell::Known(bytes), true) if *bytes >= LARGE => palette.strong(&size),
            (_, true) => size,
            (_, false) => palette.quiet(&size),
        };
        let first_rule = self.rules.first().map(String::as_str).unwrap_or_default();
        let rule = format!("{first_rule:<width$}", width = columns.rule);
        let row = format!(
            "{INDENT}{size}  {}  {}",
            palette.quiet(&rule),
            fit(&self.name, name_room)
        );

        let further_rules = self
            .rules
            .iter()
            .skip(1)
            .map(|rule| format!("{under_rule}{}", palette.quiet(rule)));
        let note = self
            .note
            .iter()
            .filter(|note| !note.is_empty())
            .flat_map(|note| note_lines(note, palette))
            .map(|line| format!("{under_rule}{line}"));
        let steps = self
            .steps
            .iter()
            .map(|step| format!("{under_rule}{}", palette.quiet(step)));

        std::iter::once(row)
            .chain(further_rules)
            .chain(note)
            .chain(steps)
            .collect()
    }
}

/// The status words, the details, and one hint on one line; several hints on the lines
/// below in the stderr hint form.
fn note_lines(note: &Note, palette: Palette) -> Vec<String> {
    let one_hint = match note.hints.as_slice() {
        [hint] => Some(hint.compact()),
        _ => None,
    };
    let quiet: Vec<String> = note.details.iter().cloned().chain(one_hint).collect();
    let status = note
        .status
        .as_ref()
        .map(|(tone, words)| palette.tone(*tone, words));
    let quiet = (!quiet.is_empty()).then(|| palette.quiet(&quiet.join("; ")));
    let first: Vec<String> = status.into_iter().chain(quiet).collect();

    let several = match note.hints.as_slice() {
        [] | [_] => Vec::new(),
        several => hint_lines(palette, several),
    };
    std::iter::once(first.join(&palette.quiet("; ")))
        .filter(|line| !line.is_empty())
        .chain(several)
        .collect()
}

/// `text`, or its end after `…` when it is longer than `room`.
fn fit(text: &str, room: Option<usize>) -> String {
    let length = width_of(text);
    match room {
        Some(room) if length > room => {
            let kept: String = text.chars().skip(length - (room - 1)).collect();
            format!("…{kept}")
        }
        Some(_) | None => text.to_owned(),
    }
}

fn width_of(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::output::ColorChoice;
    use crate::plan::{
        AggressiveItems, ItemKind, LaunchDomain, PathMatch, PlanBuilder, Reach, RunAs,
    };
    use crate::scan::{Needs, WalkSkip};
    use crate::tier::Tier;

    const HOME: &str = "/Users/me";

    fn shown() -> Shown<'static> {
        Shown {
            home: Path::new(HOME),
        }
    }

    fn parsed(plan_toml: &str) -> Plan {
        Plan::parse(&format!("version = 1\n{plan_toml}")).expect("valid plan")
    }

    fn listing(plan: &Plan, palette: Palette, width: Width) -> String {
        let left_out = LeftOut {
            skipped: &[],
            refused: &[],
            mode: &Mode::default(),
        };
        plan_listing(plan, &left_out, &shown(), palette, width).join("\n")
    }

    fn plain(plan: &Plan) -> String {
        listing(plan, Palette::plain(), Width::Unbounded)
    }

    /// `text` without its terminal color sequences.
    fn uncolored(text: &str) -> String {
        let mut plain = String::new();
        let mut rest = text;
        while let Some(start) = rest.find('\u{1b}') {
            plain.push_str(&rest[..start]);
            let end = rest[start..].find('m').expect("a color sequence ends in m");
            rest = &rest[start + end + 1..];
        }
        plain.push_str(rest);
        plain
    }

    /// A folder the user owns at `path`, in the roots, matched by `rule` as `tier`.
    fn found(path: &str, bytes: u64, rule: &str, tier: Tier) -> PathMatch {
        PathMatch {
            path: PathBuf::from(path),
            kind: ItemKind::Folder,
            size: Size::bytes(bytes),
            run_as: RunAs::User,
            reach: Reach::InRoots,
            rule: rule.to_owned(),
            tier,
        }
    }

    fn scanned(matches: impl IntoIterator<Item = PathMatch>) -> Plan {
        let mut builder = PlanBuilder::new(AggressiveItems::Skip);
        for found in matches {
            builder.add_path(found).expect("valid match");
        }
        builder.build().plan
    }

    fn deletes() -> Plan {
        let root_file = |rule| PathMatch {
            kind: ItemKind::File,
            run_as: RunAs::Sudo,
            ..found("/Users/me/x", 10, rule, Tier::Normal)
        };
        scanned([
            found(
                "/Users/me/src/app/sub/.venv",
                84_000_000,
                "rosie/python-venv",
                Tier::Aggressive,
            ),
            found(
                "/Users/me/src/app/target",
                5_000_000,
                "rosie/cargo-target",
                Tier::Normal,
            ),
            found(
                "/Users/me/src/app/node_modules",
                1_234_567_890,
                "rosie/node-modules",
                Tier::Normal,
            ),
            root_file("a/one"),
            root_file("b/two"),
        ])
    }

    #[test]
    fn folders_hold_rows_of_size_rule_and_name_a_blank_line_apart() {
        assert_eq!(
            plain(&deletes()),
            [
                "~",
                "     10 B  a/one               x",
                "           b/two",
                "           needs sudo",
                "",
                "~/src/app",
                "   1.2 GB  rosie/node-modules  node_modules",
                "   5.0 MB  rosie/cargo-target  target",
                "",
                "~/src/app/sub",
                "  84.0 MB  rosie/python-venv   .venv",
                "           skip (aggressive); to clean it: --aggressive",
            ]
            .join("\n")
        );
    }

    #[test]
    fn a_name_too_long_for_the_screen_keeps_its_end() {
        let plan = parsed(
            r#"
[[delete]]
path = "/Users/me/src/clients/acme/monorepo/packages/web/apps/some-really-long-artifact-folder-name"
type = "folder"
size = 1234567890
rules = ["x/y"]
status = "clean"
"#,
        );

        let listed = listing(&plan, Palette::plain(), Width::Columns(40));

        assert_eq!(
            listed,
            [
                "…clients/acme/monorepo/packages/web/apps",
                "  1.2 GB  x/y  …ong-artifact-folder-name",
            ]
            .join("\n")
        );
    }

    #[test]
    fn an_entry_outside_the_roots_says_so_and_lists_its_roots_hints_below() {
        let mut builder = PlanBuilder::new(AggressiveItems::Skip);
        let plist = PathMatch {
            kind: ItemKind::File,
            run_as: RunAs::Sudo,
            reach: Reach::OutsideRoots,
            ..found(
                "/Library/LaunchDaemons/com.x.plist",
                4096,
                "app/com.x",
                Tier::Normal,
            )
        };
        builder
            .add_launch_job(plist, LaunchDomain::System)
            .expect("valid plist");
        let plan = builder.build().plan;

        assert_eq!(
            plain(&plan),
            [
                "/Library/LaunchDaemons",
                "  4.1 KB  app/com.x  com.x.plist",
                "          skip (outside the roots); needs sudo; unloads system job",
                "          to allow just it:",
                "            rosie roots add /Library/LaunchDaemons/com.x.plist",
                "          or to allow its folder:",
                "            rosie roots add /Library/LaunchDaemons",
            ]
            .join("\n")
        );
    }

    #[test]
    fn a_conflict_lists_each_rule_with_its_tier() {
        let mut builder = PlanBuilder::new(AggressiveItems::Skip);
        let sides = [
            ("rosie/cargo-target", Tier::Normal),
            ("alice/rust", Tier::Aggressive),
        ];
        for (rule, tier) in sides {
            let found = PathMatch {
                path: PathBuf::from("/Users/me/api/target"),
                kind: ItemKind::Folder,
                size: Size::bytes(3_000_000_000),
                run_as: RunAs::User,
                reach: Reach::InRoots,
                rule: rule.to_owned(),
                tier,
            };
            builder.add_path(found).expect("valid match");
        }

        assert_eq!(
            plain(&builder.build().plan),
            [
                "~/api",
                "  3.0 GB  rosie/cargo-target (normal)  target",
                "          alice/rust (aggressive)",
                "          conflict",
            ]
            .join("\n")
        );
    }

    #[test]
    fn skipped_and_in_use_items_sit_with_the_plan_under_their_folders() {
        let plan = parsed(
            r#"
[[delete]]
path = "/Users/me/src/a/node_modules"
type = "folder"
size = 2000
rules = ["rosie/node-modules"]
status = "clean"
"#,
        );
        let skipped = [Skipped {
            path: PathBuf::from("/Users/me/src/a/Foo.app"),
            met_at: PathBuf::from("/Users/me/src/a/Foo.app"),
            reason: WalkSkip::Closed(Needs::BUNDLES),
        }];
        let refused = [Refused {
            path: PathBuf::from("/Users/me/src/a/target"),
            rules: vec!["rosie/cargo-target".to_owned()],
            pid: 4123,
            program: PathBuf::from("/Users/me/src/a/target/debug/app"),
            size: Size::bytes(3000),
        }];
        let left_out = LeftOut {
            skipped: &skipped,
            refused: &refused,
            mode: &Mode::default(),
        };

        let lines = plan_listing(
            &plan,
            &left_out,
            &shown(),
            Palette::plain(),
            Width::Unbounded,
        );

        assert_eq!(
            lines.join("\n"),
            [
                "~/src/a",
                "                              Foo.app",
                "          skipped: bundle; to enter it: --enter-bundles",
                "  2.0 KB  rosie/node-modules  node_modules",
                "  3.0 KB  rosie/cargo-target  target",
                "          in use: app (pid 4123)",
            ]
            .join("\n")
        );
    }

    #[test]
    fn commands_then_steps_by_hand_follow_the_folders() {
        let plan = parsed(
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
status = "skip"

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
            plain(&plan),
            [
                "~",
                "  2.0 KB  rosie/a        a",
                "",
                "commands",
                "       ?  user/homebrew  $ brew cleanup",
                "          skip (aggressive); to clean it: --aggressive",
                "       ?                 forget package com.x.pkg",
                "",
                "by hand",
                "                         login item X",
                "          Open System Settings > General > Login Items",
                "          Remove X",
            ]
            .join("\n")
        );
    }

    #[test]
    fn color_leaves_the_same_text_in_the_same_columns() {
        let plan = deletes();

        let colored = listing(
            &plan,
            Palette::for_stream(true, ColorChoice::Auto),
            Width::Unbounded,
        );

        assert!(colored.contains('\u{1b}'), "{colored}");
        assert_eq!(uncolored(&colored), plain(&plan));
    }
}
