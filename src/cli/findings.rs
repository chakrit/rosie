//! The lines the CLI prints about a scan besides the plan, from any mode, and the stats
//! lines that end a scan or a run. Every skip names its path or subject and briefly why
//! (`docs/spec/plan.md`).

use std::fmt;
use std::path::Path;

use super::args::Mode;
use crate::hint::{Hint, hint_lines};
use crate::output::{Line, Mark, Palette};
use crate::plan::Report;
use crate::scan::{Problem, Refused, Scan, Skipped, WalkSkip, WalkSkips};

/// Every skip and problem of `scan`, one per line. `mode` decides whether a walk-skip
/// line may point to a flag: only the flags the mode that produced it actually accepts.
pub(super) fn findings_lines(scan: &Scan, mode: &Mode) -> Vec<Line> {
    let skipped = scan
        .skipped
        .iter()
        .map(|skipped| walk_skip_line(skipped, mode));
    let refused = scan.refused.iter().map(refused_line);
    let reports = scan.plan.reports().iter().map(report_line);
    let skips = skipped
        .chain(refused)
        .chain(reports)
        .map(|text| Line::new(Mark::Skipped, text));
    let problems = scan.problems.iter().map(problem_line);

    skips.chain(problems).collect()
}

/// The stats lines: the plan's tallies, as scan and confirmation show them, or the
/// run's, as the end of a run shows them; then walk-skip counts.
pub(super) fn stats_lines(stats: impl fmt::Display, skips: WalkSkips) -> String {
    let stats = stats.to_string();
    match skips.total() {
        0 => stats,
        _ => format!("{stats}{skips}\n"),
    }
}

/// What would let the walk in: the roots hints for a link out of the roots, naming it as
/// `shown_path`, or the flag `mode` accepts.
pub(super) fn walk_skip_hints(skipped: &Skipped, shown_path: &Path, mode: &Mode) -> Vec<Hint> {
    match skipped.reason {
        WalkSkip::LinkOutsideRoots => Hint::allow_in_roots(shown_path),
        skip => mode.walk_skip_hint(skip).into_iter().collect(),
    }
}

fn walk_skip_line(skipped: &Skipped, mode: &Mode) -> String {
    let path = skipped.path.display();
    let reason = skipped.reason.reason();
    let hints = walk_skip_hints(skipped, &skipped.path, mode);
    match hints.as_slice() {
        [] => format!("skipped {path}: {reason}"),
        [hint] => format!("skipped {path}: {reason} ({})", hint.compact()),
        several => {
            let lines = hint_lines(Palette::plain(), several).join("\n");
            format!("skipped {path}: {reason}\n{lines}")
        }
    }
}

fn refused_line(refused: &Refused) -> String {
    format!(
        "skipped {} ({}): in use by process {} ({})",
        refused.path.display(),
        refused.size,
        refused.pid,
        refused.program.display()
    )
}

fn report_line(report: &Report) -> String {
    format!(
        "skipped {}: rosie cannot remove it; by hand: {}",
        report.subject(),
        report.steps().join("; ")
    )
}

pub(super) fn problem_line(problem: &Problem) -> Line {
    Line::new(Mark::Warning, problem.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{ModeFlags, TreeArgs};
    use crate::plan::{AgreedTiers, ItemSize, Plan, Size};
    use crate::run::{Outcome, RunStats};
    use crate::scan::Needs;

    #[test]
    fn a_report_only_item_is_a_skip_naming_its_subject_why_and_the_steps() {
        let plan = Plan::parse(
            "version = 1\n\n[[report]]\nsubject = \"login item Bar\"\nsteps = [\"Open System Settings > General > Login Items\", \"Remove Bar\"]\n",
        )
        .expect("valid plan");
        let scan = Scan {
            plan,
            skipped: Vec::new(),
            refused: Vec::new(),
            problems: Vec::new(),
            tiers: AgreedTiers::default(),
        };

        assert_eq!(
            findings_lines(&scan, &Mode::Tree(TreeArgs::default())),
            [Line::new(
                Mark::Skipped,
                "skipped login item Bar: rosie cannot remove it; by hand: Open System Settings > \
                 General > Login Items; Remove Bar"
            )]
        );
    }

    #[test]
    fn an_orphans_placeholder_skip_names_no_flag_the_mode_does_not_accept() {
        let scan = Scan {
            plan: Plan::default(),
            skipped: vec![Skipped {
                path: "/Users/x/Library/Caches/com.gone.Tool".into(),
                met_at: "/Users/x/Library/Caches/com.gone.Tool".into(),
                reason: WalkSkip::Closed(Needs::PLACEHOLDERS),
            }],
            refused: Vec::new(),
            problems: Vec::new(),
            tiers: AgreedTiers::default(),
        };

        assert_eq!(
            findings_lines(
                &scan,
                &Mode::Orphans {
                    flags: ModeFlags::default()
                }
            ),
            [Line::new(
                Mark::Skipped,
                "skipped /Users/x/Library/Caches/com.gone.Tool: cloud placeholder, not downloaded"
            )]
        );
    }

    #[test]
    fn a_tree_placeholder_skip_names_the_flag_that_mode_accepts() {
        let scan = Scan {
            plan: Plan::default(),
            skipped: vec![Skipped {
                path: "/Users/x/Library/Caches/com.gone.Tool".into(),
                met_at: "/Users/x/Library/Caches/com.gone.Tool".into(),
                reason: WalkSkip::Closed(Needs::PLACEHOLDERS),
            }],
            refused: Vec::new(),
            problems: Vec::new(),
            tiers: AgreedTiers::default(),
        };

        assert_eq!(
            findings_lines(&scan, &Mode::Tree(TreeArgs::default())),
            [Line::new(
                Mark::Skipped,
                "skipped /Users/x/Library/Caches/com.gone.Tool: cloud placeholder, not \
                 downloaded (to open it: --enter-placeholders)"
            )]
        );
    }

    #[test]
    fn run_stats_lines_end_with_the_walk_skip_counts() {
        let mut stats = RunStats::default();
        stats.record(ItemSize::Known(Size::bytes(3_400_000_000)), Outcome::Done);
        let skips: WalkSkips = [WalkSkip::Closed(Needs::MOUNTS)].into_iter().collect();

        assert_eq!(
            stats_lines(stats, skips),
            "freed 3.4 GB; done: 1 item, 3.4 GB\n\
             walk skipped 1 mount\n"
        );
    }
}
