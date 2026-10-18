//! The plan listing: a plan for reading, its deletes grouped under their folders, then
//! its commands, then the steps to take by hand (`docs/spec/plan.md#plan-listing`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::entry_text::{sides, status_hints, sudo_note};
use super::shown::Shown;
use crate::hint::{Hint, hint_lines};
use crate::output::{Bullet, Palette};
use crate::plan::{Delete, Plan, Receipt, Report, Selection, Size, Status, StatusKind, Tool};

/// Sizes from here up are shown strong.
const LARGE: Size = Size::bytes(1_000_000_000);

/// Rows start under their heading; the lines about a row start under its name.
const ROW: &str = "  ";
const BELOW_ROW: &str = "      ";

/// The listing's lines: each folder holding deletes and its rows, then the commands, then
/// the steps by hand.
pub fn plan_listing(plan: &Plan, shown: &Shown, palette: Palette) -> Vec<String> {
    let deletes = by_folder(plan.deletes())
        .into_iter()
        .flat_map(|(folder, deletes)| folder_lines(folder, &deletes, shown, palette));
    let commands = command_lines(plan.tools(), plan.receipts(), palette);
    let by_hand = by_hand_lines(plan.reports(), palette);

    deletes.chain(commands).chain(by_hand).collect()
}

// deletes

/// The deletes under each folder that holds them, folders and names in path order.
fn by_folder(deletes: &[Delete]) -> BTreeMap<&Path, Vec<&Delete>> {
    let mut folders: BTreeMap<&Path, Vec<&Delete>> = BTreeMap::new();
    for delete in deletes {
        let folder = delete.path.parent().unwrap_or(&delete.path);
        folders.entry(folder).or_default().push(delete);
    }
    for deletes in folders.values_mut() {
        deletes.sort_by(|a, b| a.path.file_name().cmp(&b.path.file_name()));
    }
    folders
}

/// The folder as a heading, then a row per delete: bullet, name, and size in columns,
/// then its rules and notes, with the hints that would make it clean below it.
fn folder_lines(
    folder: &Path,
    deletes: &[&Delete],
    shown: &Shown,
    palette: Palette,
) -> Vec<String> {
    let names: Vec<String> = deletes.iter().map(|delete| name(&delete.path)).collect();
    let sizes: Vec<String> = deletes
        .iter()
        .map(|delete| delete.size.to_string())
        .collect();
    let name_width = widest(&names);
    let size_width = widest(&sizes);

    let rows = deletes
        .iter()
        .zip(names.iter().zip(&sizes))
        .flat_map(|(delete, (name, size))| {
            let size = format!("{size:>size_width$}");
            let size = match delete.size >= LARGE {
                true => palette.strong(&size),
                false => size,
            };
            let row = format!(
                "{ROW}{} {name:<name_width$}  {size}  {}",
                palette.bullet(bullet(delete.status.kind())),
                palette.quiet(&notes(delete)),
            );
            let hints = status_hints(
                delete.status.kind(),
                &PathBuf::from(shown.path(&delete.path)),
            );
            let below = hint_text(&hints, palette)
                .into_iter()
                .map(|line| format!("{BELOW_ROW}{line}"));
            std::iter::once(row).chain(below)
        });

    std::iter::once(palette.strong(&shown.path(folder)))
        .chain(rows)
        .collect()
}

/// The last part of `path`; the whole path when it has none.
fn name(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.display().to_string(),
    }
}

/// The rules, what keeps the entry from being clean, `needs sudo`, then the jobs it
/// unloads.
fn notes(delete: &Delete) -> String {
    let rules = delete.rules.join(", ");
    let what = match &delete.status {
        Status::Clean => rules,
        Status::Keep => format!("{rules} (aggressive)"),
        Status::OutsideRoots => format!("{rules}; outside the roots"),
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
    notes.join("; ")
}

/// One hint on one quiet line; several in the stderr hint form.
fn hint_text(hints: &[Hint], palette: Palette) -> Vec<String> {
    match hints {
        [hint] => vec![palette.quiet(&hint.compact())],
        several => hint_lines(palette, several),
    }
}

fn bullet(kind: StatusKind) -> Bullet {
    match kind {
        StatusKind::Clean => Bullet::Clean,
        StatusKind::Keep => Bullet::Keep,
        StatusKind::Conflict => Bullet::Conflict,
        StatusKind::OutsideRoots => Bullet::OutsideRoots,
    }
}

// commands and steps by hand

/// `commands`, then a row per tool command and package forget: bullet, what runs, and
/// the tool's rule. No size: rosie cannot know one.
fn command_lines(tools: &[Tool], receipts: &[Receipt], palette: Palette) -> Vec<String> {
    let tools = tools.iter().map(|tool| {
        (
            tool.selection,
            format!("$ {}", tool.command.cmd()),
            tool.rule.as_str(),
        )
    });
    let forgets = receipts.iter().map(|receipt| {
        let forget = format!("forget package {}", receipt.package.as_str());
        (receipt.selection, forget, "")
    });
    let rows: Vec<(Selection, String, &str)> = tools.chain(forgets).collect();
    if rows.is_empty() {
        return Vec::new();
    }

    let texts: Vec<String> = rows.iter().map(|(_, text, _)| text.clone()).collect();
    let width = widest(&texts);
    let lines = rows.iter().map(|(selection, text, rule)| {
        let bullet = palette.bullet(bullet(Status::from(*selection).kind()));
        match rule.is_empty() {
            true => format!("{ROW}{bullet} {text}"),
            false => format!("{ROW}{bullet} {text:<width$}  {}", palette.quiet(rule)),
        }
    });

    std::iter::once(palette.strong("commands"))
        .chain(lines)
        .collect()
}

/// `by hand`, then each report's subject with its steps below it.
fn by_hand_lines(reports: &[Report], palette: Palette) -> Vec<String> {
    if reports.is_empty() {
        return Vec::new();
    }

    let lines = reports.iter().flat_map(|report| {
        let subject = format!(
            "{ROW}{} {}",
            palette.bullet(Bullet::ByHand),
            report.subject()
        );
        let steps = report
            .steps()
            .iter()
            .map(|step| format!("{BELOW_ROW}{step}"));
        std::iter::once(subject).chain(steps)
    });

    std::iter::once(palette.strong("by hand"))
        .chain(lines)
        .collect()
}

fn widest(cells: &[String]) -> usize {
    cells
        .iter()
        .map(|cell| cell.chars().count())
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::output::ColorChoice;
    use crate::plan::{AggressiveItems, ItemKind, PathMatch, PlanBuilder, Reach, RunAs, Size};
    use crate::tier::Tier;

    const HOME: &str = "/Users/me";

    fn listing(plan_toml: &str, palette: Palette) -> String {
        let plan = Plan::parse(&format!("version = 1\n{plan_toml}")).expect("valid plan");
        let shown = Shown {
            home: Path::new(HOME),
        };
        plan_listing(&plan, &shown, palette).join("\n")
    }

    fn plain(plan_toml: &str) -> String {
        listing(plan_toml, Palette::plain())
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

    const DELETES: &str = r#"
[[delete]]
path = "/Users/me/src/app/sub/.venv"
type = "folder"
size = 84000000
rules = ["rosie/python-venv"]
status = "keep"

[[delete]]
path = "/Users/me/src/app/target"
type = "folder"
size = 5000000
rules = ["rosie/cargo-target"]
status = "clean"

[[delete]]
path = "/Users/me/src/app/node_modules"
type = "folder"
size = 1234567890
rules = ["rosie/node-modules"]
status = "clean"

[[delete]]
path = "/Users/me/x"
type = "file"
size = 10
rules = ["a/one", "b/two"]
status = "clean"
sudo = true
"#;

    #[test]
    fn deletes_sit_under_their_folders_by_name_with_bullet_size_and_rules() {
        assert_eq!(
            plain(DELETES),
            [
                "~",
                "  ● x  10 B  a/one, b/two; needs sudo",
                "~/src/app",
                "  ● node_modules  1.2 GB  rosie/node-modules",
                "  ● target        5.0 MB  rosie/cargo-target",
                "~/src/app/sub",
                "  ○ .venv  84.0 MB  rosie/python-venv (aggressive)",
                "      to clean it: --aggressive",
            ]
            .join("\n")
        );
    }

    #[test]
    fn an_outside_roots_entry_says_so_and_lists_its_roots_hints_below() {
        let listed = plain(
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

        assert_eq!(
            listed,
            [
                "/Library/LaunchDaemons",
                "  ✗ com.x.plist  4.1 KB  app/com.x; outside the roots; needs sudo; unloads system job",
                "      to allow just it:",
                "        rosie roots add /Library/LaunchDaemons/com.x.plist",
                "      or to allow its folder:",
                "        rosie roots add /Library/LaunchDaemons",
            ]
            .join("\n")
        );
    }

    #[test]
    fn a_conflict_shows_both_sides_and_how_to_settle_it() {
        let mut builder = PlanBuilder::new(AggressiveItems::Keep);
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
        let plan = builder.build().plan;
        let shown = Shown {
            home: Path::new(HOME),
        };

        let listed = plan_listing(&plan, &shown, Palette::plain()).join("\n");

        assert_eq!(
            listed,
            [
                "~/api",
                "  ! target  3.0 GB  rosie/cargo-target (normal) vs alice/rust (aggressive)",
                "      choose clean or keep in the plan",
            ]
            .join("\n")
        );
    }

    #[test]
    fn commands_then_steps_by_hand_follow_the_deletes() {
        let listed = plain(
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
            listed,
            [
                "~",
                "  ● a  2.0 KB  rosie/a",
                "commands",
                "  ○ $ brew cleanup            user/homebrew",
                "  ● forget package com.x.pkg",
                "by hand",
                "  · login item X",
                "      Open System Settings > General > Login Items",
                "      Remove X",
            ]
            .join("\n")
        );
    }

    #[test]
    fn color_leaves_the_same_text_in_the_same_columns() {
        let colored = listing(DELETES, Palette::for_stream(true, ColorChoice::Auto));

        assert!(colored.contains('\u{1b}'), "{colored}");
        assert_eq!(uncolored(&colored), plain(DELETES));
    }
}
