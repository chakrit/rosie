//! `rosie check <path>…`: one line per path saying what a plan would do with it
//! (`docs/spec/cli.md#check`).
//!
//! Each path is scanned by [`crate::scan::Scanner::path`], which walks toward it and
//! through it as a `tree` walk does, so its verdict is the plan's. A path that would be
//! an entry, or lies inside one, takes the plan's status word; any other takes a scan
//! report word: `in use`, `skipped`, or `no rule`.

use std::path::Path;

use super::args::{CheckArgs, flag};
use super::console::{Console, Finish};
use super::scan::{walk_with_rule_flags, walk_with_scan_flags};
use super::session::Session;
use super::{Error, ExitStatus};
use crate::fs::{self, Backend, EnterLinks, FsOp, UserInput};
use crate::packs::Network;
use crate::plan::{Delete, Disagreement, PlanStats, RunAs, Status, Tally, Twin, WalkSkip};
use crate::scan::{self, PathScan, Refused, Scan, Skipped};
use crate::shell;

const CLEAN: &str = "clean";
const IN_USE: &str = "in use";
const SKIPPED: &str = "skipped";
const NO_RULE: &str = "no rule";

impl<'a, B: Backend + Sync, N: Network, C: Console> Session<'a, B, N, C> {
    /// Prints one line per path, in columns, after scanning them all with one context,
    /// so one running-process snapshot serves every path. Exits 0 when any path would be
    /// cleaned, else 1.
    pub fn check(&mut self, args: CheckArgs) -> Result<ExitStatus, Error> {
        let config = self.config()?;
        let walk = walk_with_scan_flags(config.config().walk, &args.flags);
        let walk = walk_with_rule_flags(walk, &args.rules);
        let rules = self.rules(&args.rules.only)?;
        let ctx = self.roots_ctx(config.config())?;
        let scanner = self.scanner(ctx, rules, walk, args.flags.aggressive_items());

        let mut lines = Vec::new();
        for typed in &args.paths {
            let typed = self.absolute(typed);
            let progress = self.console.progress();
            let scanned = scanner.path(&typed, &progress);
            progress.finish()?;
            let line = match scanned {
                Ok(scanned) => self.judged(&scanned)?,
                Err(scan::Error::Missing { path }) => Line {
                    status: NO_RULE,
                    path: self.tilde().path(&path),
                    reason: "does not exist".to_owned(),
                },
                Err(scan::Error::PathRefused(error)) => Line {
                    status: SKIPPED,
                    path: self.tilde().path(&typed),
                    reason: self.refusal_reason(&error),
                },
                Err(error) => return Err(error.into()),
            };
            lines.push(line);
        }

        let cleaned = lines.iter().any(|line| line.status == CLEAN);
        for text in layout(&lines) {
            self.say(&text)?;
        }
        Ok(ExitStatus::failed_if(!cleaned))
    }

    /// The line for a scanned path; the scan's problems go to stderr as `scan` prints
    /// them.
    fn judged(&mut self, scanned: &PathScan) -> Result<Line, Error> {
        for problem in &scanned.scan.problems {
            self.note(&format!("problem: {problem}"))?;
        }

        let verdict = judge(&scanned.path, &scanned.scan);
        let shown = self.tilde();
        Ok(verdict.line(&scanned.path, &shown))
    }

    /// Why a typed path was refused. A path through a symlink names the link and where
    /// it leads, and the `enter_links` level that allows it; any other refusal is the
    /// filesystem's own words.
    fn refusal_reason(&self, error: &fs::Error) -> String {
        let fs::Error::Symlink { link, .. } = error else {
            return error.to_string();
        };
        let shown = self.tilde();
        match UserInput::path(link, EnterLinks::Roots).perform(&self.own) {
            Ok(target) => format!(
                "through symlink {} → {}; enter_links = \"roots\" allows it",
                shown.path(link),
                shown.path(&target)
            ),
            Err(unfollowed) => format!("{error}; {unfollowed}"),
        }
    }

    /// How this session prints paths: under the injected home as `~`.
    fn tilde(&self) -> Shown<'a> {
        Shown {
            home: self.env.paths.home(),
        }
    }
}

/// One printed line: status, path, and the reason with any hint.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    status: &'static str,
    path: String,
    reason: String,
}

/// The lines in columns: each column as wide as its widest entry, two spaces apart.
fn layout(lines: &[Line]) -> Vec<String> {
    let width = |column: fn(&Line) -> usize| lines.iter().map(column).max().unwrap_or(0);
    let status_width = width(|line| line.status.chars().count());
    let path_width = width(|line| line.path.chars().count());

    lines
        .iter()
        .map(|line| {
            format!(
                "{:<status_width$}  {:<path_width$}  {}",
                line.status, line.path, line.reason
            )
        })
        .collect()
}

/// Paths as `check` prints them: under home as `~`.
struct Shown<'h> {
    home: &'h Path,
}

impl Shown<'_> {
    fn path(&self, path: &Path) -> String {
        match path.strip_prefix(self.home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }

    /// `path` as a shell word to paste, with home still written `~`.
    fn word(&self, path: &Path) -> String {
        match path.strip_prefix(self.home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Ok(rest) => format!("~/{}", shell::quote_word(&rest.to_string_lossy())),
            Err(_) => shell::quote_word(&path.to_string_lossy()),
        }
    }
}

// verdicts

/// What the plan would do with a path, judged from the scan of it.
#[derive(Debug)]
enum Verdict<'s> {
    /// A plan entry at the path or holding it.
    Entry {
        delete: &'s Delete,
        planned: Planned<'s>,
        place: Place,
    },
    /// An item a process runs from, at the path or holding it.
    InUse {
        refused: &'s Refused,
        place: Place,
    },
    /// A walk skip at the path or above it.
    Skipped(&'s Skipped),
    /// Plan entries strictly inside the path.
    Holds(PlanStats),
    /// No plan entry inside the path, but items refused or skipped inside it.
    HoldsUnplanned {
        in_use: usize,
        skipped: usize,
    },
    NoRule,
}

/// A plan entry's status, with the tier its rules agree on where it shows.
#[derive(Debug)]
enum Planned<'s> {
    Clean(Twin),
    Keep,
    OutsideRoots,
    Conflict(&'s Disagreement),
}

/// Where the item a verdict names sits against the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    At,
    Inside,
}

/// The verdict for `path`, a real path, from its scan, most telling first: an item a
/// process runs from at or above it, a plan entry at or above it, a walk skip at or
/// above it, the plan entries inside it, the refused and skipped items inside it, and
/// otherwise no rule.
fn judge<'s>(path: &Path, scan: &'s Scan) -> Verdict<'s> {
    let place = |at: &Path| match at == path {
        true => Place::At,
        false => Place::Inside,
    };
    let holds_path = |at: &Path| path.starts_with(at);
    let inside_path = |at: &Path| at != path && at.starts_with(path);

    let refused = scan
        .refused
        .iter()
        .find(|refused| holds_path(&refused.path));
    if let Some(refused) = refused {
        let place = place(&refused.path);
        return Verdict::InUse { refused, place };
    }
    let deletes = scan.plan.deletes();
    if let Some(delete) = deletes.iter().find(|delete| holds_path(&delete.path)) {
        let planned = planned(delete, scan);
        let place = place(&delete.path);
        return Verdict::Entry {
            delete,
            planned,
            place,
        };
    }
    if let Some(skipped) = scan.skipped.iter().find(|skip| holds_path(&skip.path)) {
        return Verdict::Skipped(skipped);
    }

    let inside: Vec<&Delete> = deletes
        .iter()
        .filter(|delete| inside_path(&delete.path))
        .collect();
    if !inside.is_empty() {
        return Verdict::Holds(PlanStats::of_deletes(inside));
    }
    let in_use = scan
        .refused
        .iter()
        .filter(|refused| inside_path(&refused.path))
        .count();
    let skipped = scan
        .skipped
        .iter()
        .filter(|skip| inside_path(&skip.path))
        .count();
    match (in_use, skipped) {
        (0, 0) => Verdict::NoRule,
        _ => Verdict::HoldsUnplanned { in_use, skipped },
    }
}

/// `delete`'s status, with the tier its rules agree on when it is `clean`.
fn planned<'s>(delete: &'s Delete, scan: &Scan) -> Planned<'s> {
    match &delete.status {
        Status::Clean => {
            let tier = scan.tiers.of(&delete.path);
            Planned::Clean(tier.expect("the plan builder records the tier of each clean entry"))
        }
        Status::Keep => Planned::Keep,
        Status::OutsideRoots => Planned::OutsideRoots,
        Status::Conflict(disagreement) => Planned::Conflict(disagreement),
    }
}

impl Verdict<'_> {
    /// The line for `path`, the real path the verdict is about.
    fn line(&self, path: &Path, shown: &Shown) -> Line {
        let (status, reason) = match self {
            Verdict::Entry {
                delete,
                planned,
                place,
            } => (
                delete.status.label(),
                entry_reason(delete, planned, *place, shown),
            ),
            Verdict::InUse { refused, place } => (IN_USE, in_use_reason(refused, *place, shown)),
            Verdict::Skipped(skipped) => (SKIPPED, skip_reason(skipped, shown)),
            Verdict::Holds(stats) => holds(stats, path, shown),
            Verdict::HoldsUnplanned { in_use, skipped } => {
                holds_unplanned(*in_use, *skipped, path, shown)
            }
            Verdict::NoRule => (NO_RULE, "no rule matches it or anything inside".to_owned()),
        };
        Line {
            status,
            path: shown.path(path),
            reason,
        }
    }
}

/// `rosie/node-modules (normal)` at the entry, `inside <entry> (<rules>)` below it,
/// then what would change its status, and `needs sudo` when it is not the user's.
fn entry_reason(delete: &Delete, planned: &Planned, place: Place, shown: &Shown) -> String {
    let rules = delete.rules.join(", ");
    let what = match (place, planned) {
        (Place::Inside, _) => Some(format!("inside {} ({rules})", shown.path(&delete.path))),
        (Place::At, Planned::Clean(tier)) => Some(format!("{rules} ({})", tier_label(*tier))),
        (Place::At, Planned::Keep) => Some(format!("{rules} (aggressive)")),
        (Place::At, Planned::OutsideRoots) => None,
        (Place::At, Planned::Conflict(disagreement)) => Some(sides(disagreement)),
    };
    let hint = match planned {
        Planned::Clean(_) => None,
        Planned::Keep => Some("--aggressive cleans it".to_owned()),
        Planned::OutsideRoots => Some(format!(
            "to allow it: rosie roots add {}",
            shown.word(&delete.path)
        )),
        Planned::Conflict(_) => Some("choose clean or keep in the plan".to_owned()),
    };
    let sudo = (delete.run_as == RunAs::Sudo).then(|| "needs sudo".to_owned());

    let parts: Vec<String> = [what, hint, sudo].into_iter().flatten().collect();
    parts.join("; ")
}

/// `rosie/a (normal) vs alice/b (aggressive)`.
fn sides(disagreement: &Disagreement) -> String {
    format!(
        "{} (normal) vs {} (aggressive)",
        disagreement.normal().join(", "),
        disagreement.aggressive().join(", ")
    )
}

fn tier_label(tier: Twin) -> &'static str {
    match tier {
        Twin::Normal => "normal",
        Twin::Aggressive => "aggressive",
    }
}

/// `running: Slack (pid 4123)`, after `inside <item> (<rules>)` below the item.
fn in_use_reason(refused: &Refused, place: Place, shown: &Shown) -> String {
    let program = refused
        .program
        .file_name()
        .unwrap_or(refused.program.as_os_str());
    let running = format!(
        "running: {} (pid {})",
        program.to_string_lossy(),
        refused.pid
    );
    match place {
        Place::At => running,
        Place::Inside => format!(
            "inside {} ({}); {running}",
            shown.path(&refused.path),
            refused.rules.join(", ")
        ),
    }
}

/// The skip's reason, then the flags that would enter it or the `roots add` that would
/// let it in.
fn skip_reason(skipped: &Skipped, shown: &Shown) -> String {
    let reason = skipped.reason.reason();
    let hint = match skipped.reason {
        WalkSkip::Closed(needs) => {
            let flags: Vec<&str> = needs.settings().map(flag).collect();
            match flags.split_last() {
                Some((only, [])) => Some(format!("{only} enters it")),
                Some((last, init)) => Some(format!("{} and {last} enter it", init.join(", "))),
                None => None,
            }
        }
        WalkSkip::LinkOutsideRoots => Some(format!(
            "to allow it: rosie roots add {}",
            shown.word(&skipped.path)
        )),
        WalkSkip::SealedMount
        | WalkSkip::SealedPlaceholder
        | WalkSkip::Denied
        | WalkSkip::LinkWalked
        | WalkSkip::LinkLoop
        | WalkSkip::LinkDangling => None,
    };
    match hint {
        Some(hint) => format!("{reason}; {hint}"),
        None => reason,
    }
}

/// The most consequential status among the entries inside `path`, and `contains 3
/// items: 2 clean (1.4 GB), 1 keep (210 MB); see rosie scan tree <path>`.
fn holds(stats: &PlanStats, path: &Path, shown: &Shown) -> (&'static str, String) {
    let buckets = [
        ("conflict", stats.conflicts),
        (CLEAN, stats.clean),
        ("keep", stats.keep),
        ("outside-roots", stats.outside_roots),
    ];
    let present: Vec<(&'static str, Tally)> = buckets
        .into_iter()
        .filter(|(_, tally)| !tally.is_empty())
        .collect();
    let &(status, _) = present
        .first()
        .expect("a folder holding plan entries holds at least one status");

    let items: usize = present.iter().map(|(_, tally)| tally.items).sum();
    let noun = match items {
        1 => "item",
        _ => "items",
    };
    let counts: Vec<String> = present
        .iter()
        .map(|(status, tally)| format!("{} {status} ({})", tally.items, tally.size))
        .collect();
    let reason = format!(
        "contains {items} {noun}: {}; see rosie scan tree {}",
        counts.join(", "),
        shown.word(path)
    );
    (status, reason)
}

/// `in use` when anything inside runs, else `skipped`, with `contains 1 in use, 2
/// skipped; see rosie scan tree <path>`.
fn holds_unplanned(
    in_use: usize,
    skipped: usize,
    path: &Path,
    shown: &Shown,
) -> (&'static str, String) {
    let status = match in_use {
        0 => SKIPPED,
        _ => IN_USE,
    };
    let counts = [(in_use, IN_USE), (skipped, SKIPPED)];
    let parts: Vec<String> = counts
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, word)| format!("{count} {word}"))
        .collect();
    let reason = format!(
        "contains {}; see rosie scan tree {}",
        parts.join(", "),
        shown.word(path)
    );
    (status, reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(status: &'static str, path: &str, reason: &str) -> Line {
        Line {
            status,
            path: path.to_owned(),
            reason: reason.to_owned(),
        }
    }

    #[test]
    fn columns_are_as_wide_as_their_widest_entry_two_spaces_apart() {
        let lines = [
            line("clean", "~/a/node_modules", "rosie/node-modules (normal)"),
            line("outside-roots", "~/b", "to allow it: rosie roots add ~/b"),
        ];

        assert_eq!(
            layout(&lines),
            [
                "clean          ~/a/node_modules  rosie/node-modules (normal)",
                "outside-roots  ~/b               to allow it: rosie roots add ~/b",
            ]
        );
    }

    #[test]
    fn paths_under_home_print_from_tilde_and_paste_as_shell_words() {
        let shown = Shown {
            home: Path::new("/Users/me"),
        };

        assert_eq!(shown.path(Path::new("/Users/me")), "~");
        assert_eq!(shown.path(Path::new("/Users/me/My App")), "~/My App");
        assert_eq!(shown.path(Path::new("/Users/meme")), "/Users/meme");
        assert_eq!(shown.word(Path::new("/Users/me/My App")), "~/'My App'");
    }
}
