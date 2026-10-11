//! `rosie check <path>…`: one line per path saying what a plan would do with it
//! (`docs/spec/cli.md#check`).
//!
//! Each path is scanned by [`crate::scan::Scanner::path`], which walks toward it and
//! through it as a `tree` walk does, so its verdict is the plan's. A path that would be
//! an entry, or lies inside one, takes the plan's status word; any other takes a scan
//! report word: `in use`, `skipped`, or `no rule`.
//!
//! An `.app` bundle the walk claims nothing at or above, and skips only as a closed
//! bundle, is judged instead from the plan [`apps::scan_app`] makes for it, as `rosie
//! scan app` would plan it; a process running from it makes it `in use`, and any other
//! refusal or error of that scan makes it `skipped` with the error's text.

use std::path::Path;

use super::args::{CheckArgs, flag};
use super::console::{Console, Finish};
use super::findings::problem_line;
use super::scan::{walk_with_rule_flags, walk_with_scan_flags};
use super::session::Session;
use super::{Error, ExitStatus};
use crate::apps;
use crate::config::Walk;
use crate::fs::{self, Backend, EnterLinks, FsContext, FsOp, UserInput};
use crate::packs::Network;
use crate::plan::{
    AggressiveItems, Delete, Disagreement, Needs, PlanStats, RunAs, Status, StatusKind, Tally,
    WalkSkip,
};
use crate::scan::{self, PathScan, Problem, Refused, Scan, Scanner, Skipped};
use crate::shell;
use crate::tier::Tier;

impl<'a, B: Backend + Sync, N: Network, C: Console> Session<'a, B, N, C> {
    /// Prints one line per path, in columns, after scanning them all with one context,
    /// so one running-process snapshot serves every path. Exits 0 when any path would be
    /// cleaned, else 1.
    pub fn check(&mut self, args: CheckArgs) -> Result<ExitStatus, Error> {
        let config = self.config()?;
        let aggressive = args.flags.aggressive_items();
        let app_mode = AppMode {
            walk: walk_with_scan_flags(config.config().walk, &args.flags),
            aggressive,
        };
        let walk = walk_with_rule_flags(app_mode.walk, &args.rules);
        let rules = self.rules(&args.rules.only)?;
        let ctx = self.roots_ctx(config.config())?;
        let scanner = self.scanner(ctx, rules, walk, aggressive);

        let mut lines = Vec::new();
        for typed in &args.paths {
            let typed = self.absolute(typed);
            let progress = self.console.progress();
            let scanned = scanner.path(&typed, &progress);
            progress.finish()?;
            let line = match scanned {
                Ok(scanned) => self.judged(&scanned, &scanner, app_mode)?,
                Err(scan::Error::Missing { path }) => Line {
                    word: Word::NoRule,
                    path: self.tilde().path(&path),
                    reason: "does not exist".to_owned(),
                },
                Err(scan::Error::PathRefused(error)) => Line {
                    word: Word::Skipped,
                    path: self.tilde().path(&typed),
                    reason: self.refusal_reason(&error),
                },
                Err(error) => return Err(error.into()),
            };
            lines.push(line);
        }

        let cleaned = lines
            .iter()
            .any(|line| matches!(line.word, Word::Plan(StatusKind::Clean)));
        for text in layout(&lines) {
            self.say(&text)?;
        }
        Ok(ExitStatus::failed_if(!cleaned))
    }

    /// The line for a scanned path, from app mode when the walk leaves the path to it;
    /// the scan's problems go to stderr as `scan` prints them.
    fn judged(
        &mut self,
        scanned: &PathScan,
        scanner: &Scanner<&'a B>,
        app_mode: AppMode,
    ) -> Result<Line, Error> {
        for problem in &scanned.scan.problems {
            self.note(&problem_line(problem))?;
        }
        let path = &scanned.path;
        let verdict = judge(path, &scanned.scan);
        let shown = self.tilde();
        if !leaves_to_app_mode(&verdict, path) {
            return Ok(verdict.line(path, &shown));
        }

        let app_line = self.app_line(scanner.ctx(), path, app_mode);
        Ok(app_line.unwrap_or_else(|| verdict.line(path, &shown)))
    }

    /// The line for the `.app` at `path` from the plan `rosie scan app` makes for it:
    /// `in use` when that scan refuses the bundle for a process running from it, and
    /// `skipped` with the error's text when it refuses or cannot read the bundle for any
    /// other reason. `None` when the path is not an app after all.
    fn app_line(&self, ctx: &FsContext<&'a B>, path: &Path, app_mode: AppMode) -> Option<Line> {
        let paths = &self.env.paths;
        let shown = self.tilde();

        let scanned = apps::scan_app(ctx, path, paths, app_mode.aggressive, app_mode.walk);

        let (word, reason) = match scanned {
            Ok(scan) => return Some(judge(path, &scan).line(path, &shown)),
            Err(apps::Error::NotAnApp { .. }) => return None,
            Err(apps::Error::Refused {
                source: fs::Error::Running { pid, program, .. },
                ..
            }) => (Word::InUse, running(&program, pid)),
            Err(error) => (Word::Skipped, error.to_string()),
        };
        Some(Line {
            word,
            path: shown.path(path),
            reason,
        })
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

/// The settings `rosie scan app` runs with under the same flags.
#[derive(Clone, Copy)]
struct AppMode {
    walk: Walk,
    aggressive: AggressiveItems,
}

/// One printed line: status, path, and the reason with any hint.
struct Line {
    word: Word,
    path: String,
    reason: String,
}

/// A line's status: a plan word for a would-be entry, a scan report word otherwise.
#[derive(Clone, Copy)]
enum Word {
    Plan(StatusKind),
    InUse,
    Skipped,
    NoRule,
}

impl Word {
    fn label(self) -> &'static str {
        match self {
            Word::Plan(kind) => kind.label(),
            Word::InUse => "in use",
            Word::Skipped => "skipped",
            Word::NoRule => "no rule",
        }
    }
}

/// The lines in columns: each column as wide as its widest entry, two spaces apart.
fn layout(lines: &[Line]) -> Vec<String> {
    let width = |column: fn(&Line) -> usize| lines.iter().map(column).max().unwrap_or(0);
    let status_width = width(|line| line.word.label().chars().count());
    let path_width = width(|line| line.path.chars().count());

    lines
        .iter()
        .map(|line| {
            format!(
                "{:<status_width$}  {:<path_width$}  {}",
                line.word.label(),
                line.path,
                line.reason
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
    /// A failed read at the path or above it, so the walk never judged the path.
    Unread(&'s Problem),
    /// Plan entries strictly inside the path.
    Holds(PlanStats),
    /// No plan entry inside the path, but items refused, skipped, or unread inside it;
    /// `skipped` counts the walk skips and the failed reads.
    HoldsUnplanned {
        in_use: usize,
        skipped: usize,
    },
    NoRule,
}

/// A plan entry's status, with the tier its rules agree on where it shows.
#[derive(Debug)]
enum Planned<'s> {
    Clean(Tier),
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
/// above it, a failed read at or above it, the plan entries inside it, the refused,
/// skipped, and unread items inside it, and otherwise no rule.
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
    if let Some(skipped) = scan.skipped.iter().find(|skip| holds_path(&skip.met_at)) {
        return Verdict::Skipped(skipped);
    }
    let unread = scan
        .problems
        .iter()
        .find(|problem| problem.met_at().is_some_and(holds_path));
    if let Some(problem) = unread {
        return Verdict::Unread(problem);
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
    let walk_skips = scan
        .skipped
        .iter()
        .filter(|skip| inside_path(&skip.met_at))
        .count();
    let unread_inside = scan
        .problems
        .iter()
        .filter(|problem| problem.met_at().is_some_and(inside_path))
        .count();
    let skipped = walk_skips + unread_inside;
    match (in_use, skipped) {
        (0, 0) => Verdict::NoRule,
        _ => Verdict::HoldsUnplanned { in_use, skipped },
    }
}

/// Whether the walk leaves `path` to app mode: it is named as an `.app`, nothing at or
/// above it is a plan entry, in use, or unread, and the walk skipped nothing above it,
/// nor it for any reason but that it is a closed bundle.
fn leaves_to_app_mode(verdict: &Verdict, path: &Path) -> bool {
    let unclaimed = match verdict {
        Verdict::Entry { .. } | Verdict::InUse { .. } | Verdict::Unread(_) => false,
        Verdict::Skipped(skipped) => {
            skipped.met_at == path && skipped.reason == WalkSkip::Closed(Needs::BUNDLES)
        }
        Verdict::Holds(_) | Verdict::HoldsUnplanned { .. } | Verdict::NoRule => true,
    };
    unclaimed && apps::is_app_path(path)
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
        let (word, reason) = match self {
            Verdict::Entry {
                delete,
                planned,
                place,
            } => (
                Word::Plan(delete.status.kind()),
                entry_reason(delete, planned, *place, shown),
            ),
            Verdict::InUse { refused, place } => {
                (Word::InUse, in_use_reason(refused, *place, shown))
            }
            Verdict::Skipped(skipped) => (Word::Skipped, skip_reason(skipped)),
            Verdict::Unread(problem) => (Word::Skipped, problem.to_string()),
            Verdict::Holds(stats) => holds(stats, path, shown),
            Verdict::HoldsUnplanned { in_use, skipped } => {
                holds_unplanned(*in_use, *skipped, path, shown)
            }
            Verdict::NoRule => (
                Word::NoRule,
                "no rule matches it or anything inside".to_owned(),
            ),
        };
        Line {
            word,
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
        (Place::At, Planned::Clean(tier)) => Some(format!("{rules} ({})", tier.label())),
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

/// `running: Slack (pid 4123)`, after `inside <item> (<rules>)` below the item.
fn in_use_reason(refused: &Refused, place: Place, shown: &Shown) -> String {
    let running = running(&refused.program, refused.pid);
    match place {
        Place::At => running,
        Place::Inside => format!(
            "inside {} ({}); {running}",
            shown.path(&refused.path),
            refused.rules.join(", ")
        ),
    }
}

/// `running: Slack (pid 4123)`, naming the program by its file name.
fn running(program: &Path, pid: u32) -> String {
    let name = program.file_name().unwrap_or(program.as_os_str());
    format!("running: {} (pid {pid})", name.to_string_lossy())
}

/// The skip's reason, then the flags that would enter it.
fn skip_reason(skipped: &Skipped) -> String {
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
        WalkSkip::LinkOutsideRoots
        | WalkSkip::SealedMount
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
fn holds(stats: &PlanStats, path: &Path, shown: &Shown) -> (Word, String) {
    let buckets = [
        (StatusKind::Conflict, stats.conflicts),
        (StatusKind::Clean, stats.clean),
        (StatusKind::Keep, stats.keep),
        (StatusKind::OutsideRoots, stats.outside_roots),
    ];
    let present: Vec<(StatusKind, Tally)> = buckets
        .into_iter()
        .filter(|(_, tally)| !tally.is_empty())
        .collect();
    let &(kind, _) = present
        .first()
        .expect("a folder holding plan entries holds at least one status");

    let items: usize = present.iter().map(|(_, tally)| tally.items).sum();
    let noun = match items {
        1 => "item",
        _ => "items",
    };
    let counts: Vec<String> = present
        .iter()
        .map(|(kind, tally)| format!("{} {} ({})", tally.items, kind.label(), tally.size))
        .collect();
    let reason = format!(
        "contains {items} {noun}: {}; see rosie scan tree {}",
        counts.join(", "),
        shown.word(path)
    );
    (Word::Plan(kind), reason)
}

/// `in use` when anything inside runs, else `skipped`, with `contains 1 in use, 2
/// skipped; see rosie scan tree <path>`.
fn holds_unplanned(in_use: usize, skipped: usize, path: &Path, shown: &Shown) -> (Word, String) {
    let word = match in_use {
        0 => Word::Skipped,
        _ => Word::InUse,
    };
    let counts = [(in_use, Word::InUse), (skipped, Word::Skipped)];
    let parts: Vec<String> = counts
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, counted)| format!("{count} {}", counted.label()))
        .collect();
    let reason = format!(
        "contains {}; see rosie scan tree {}",
        parts.join(", "),
        shown.word(path)
    );
    (word, reason)
}

#[cfg(test)]
mod tests {
    use super::*;

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
