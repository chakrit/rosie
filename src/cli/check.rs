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
//! scan app` would plan it; a process running from it makes it `in use`, any other
//! refusal of the bundle, or a bundle that cannot be read, makes it `skipped` with the
//! error's text, and any other error of that scan fails `check` as it fails `scan`.

use std::path::Path;

use super::args::{CheckArgs, flag};
use super::console::{Console, Finish};
use super::findings::problem_line;
use super::session::Session;
use super::{Error, ExitStatus};
use crate::apps;
use crate::fs::{self, Backend, EnterLinks, FsContext, FsOp, UserInput};
use crate::packs::Network;
use crate::plan::{Delete, Disagreement, PlanStats, RunAs, StatusKind, Tally};
use crate::scan::{
    self, Needs, PathScan, PathVerdict, Place, Planned, Refused, Scanner, Settings, Skipped,
    WalkSkip,
};
use crate::shell;

impl<'a, B: Backend + Sync, N: Network, C: Console> Session<'a, B, N, C> {
    /// Prints one line per path, in columns, after scanning them all with one context,
    /// so one running-process snapshot serves every path. Exits 0 when any path would be
    /// cleaned, else 1.
    pub fn check(&mut self, args: CheckArgs) -> Result<ExitStatus, Error> {
        let config = self.config()?;
        // The settings `rosie scan app` runs with under the same flags.
        let app_mode = args.flags.settings(config.walk);
        let scanner = self.rule_scanner(&config, app_mode, &args.rules)?;

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
        app_mode: Settings,
    ) -> Result<Line, Error> {
        for problem in &scanned.scan.problems {
            let line = problem_line(problem);
            self.note(line.mark, &line.text)?;
        }
        let path = &scanned.path;
        let verdict = scanned.scan.verdict(path);
        let shown = self.tilde();
        if !leaves_to_app_mode(&verdict, path) {
            return Ok(line(&verdict, path, &shown));
        }

        self.app_line(scanner.ctx(), path, app_mode, &verdict)
    }

    /// The line for the `.app` at `path` from the plan `rosie scan app` makes for it:
    /// `in use` when that scan refuses the bundle for a process running from it, and
    /// `skipped` with the error's text when it refuses or cannot read the bundle for any
    /// other reason. The walk's `verdict` decides when the path is not an app after all;
    /// any other error of that scan is not about the bundle and propagates.
    fn app_line(
        &self,
        ctx: &FsContext<&'a B>,
        path: &Path,
        app_mode: Settings,
        verdict: &PathVerdict,
    ) -> Result<Line, Error> {
        let shown = self.tilde();

        let scanned = apps::scan_app(ctx, path, &app_mode);

        let (word, reason) = match scanned {
            Ok(scan) => return Ok(line(&scan.verdict(path), path, &shown)),
            Err(apps::Error::NotAnApp { .. }) => return Ok(line(verdict, path, &shown)),
            Err(apps::Error::Refused {
                source: fs::Error::Running { pid, program, .. },
                ..
            }) => (Word::InUse, running(&program, pid)),
            Err(
                error @ (apps::Error::Refused { .. }
                | apps::Error::UnreadableBundle { .. }
                | apps::Error::NoInfoPlist { .. }
                | apps::Error::MissingKey { .. }
                | apps::Error::InvalidBundleId { .. }
                | apps::Error::Placeholder { .. }
                | apps::Error::Mount { .. }),
            ) => (Word::Skipped, error.to_string()),
            Err(error) => return Err(error.into()),
        };
        Ok(Line {
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

/// Whether the walk leaves `path` to app mode: it is named as an `.app`, nothing at or
/// above it is a plan entry, in use, or unread, and the walk skipped nothing above it,
/// nor it for any reason but that it is a closed bundle.
fn leaves_to_app_mode(verdict: &PathVerdict, path: &Path) -> bool {
    let unclaimed = match verdict {
        PathVerdict::Entry { .. } | PathVerdict::InUse { .. } | PathVerdict::Unread(_) => false,
        PathVerdict::Skipped(skipped) => {
            skipped.met_at == path && skipped.reason == WalkSkip::Closed(Needs::BUNDLES)
        }
        PathVerdict::Holds(_) | PathVerdict::HoldsUnplanned { .. } | PathVerdict::NoRule => true,
    };
    unclaimed && apps::is_app_path(path)
}

/// The line for `path`, the real path `verdict` is about.
fn line(verdict: &PathVerdict, path: &Path, shown: &Shown) -> Line {
    let (word, reason) = match verdict {
        PathVerdict::Entry {
            delete,
            planned,
            place,
        } => (
            Word::Plan(delete.status.kind()),
            entry_reason(delete, planned, *place, shown),
        ),
        PathVerdict::InUse { refused, place } => {
            (Word::InUse, in_use_reason(refused, *place, shown))
        }
        PathVerdict::Skipped(skipped) => (Word::Skipped, skip_reason(skipped)),
        PathVerdict::Unread(problem) => (Word::Skipped, problem.to_string()),
        PathVerdict::Holds(stats) => holds(stats, path, shown),
        PathVerdict::HoldsUnplanned { in_use, skipped } => {
            holds_unplanned(*in_use, *skipped, path, shown)
        }
        PathVerdict::NoRule => (
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
