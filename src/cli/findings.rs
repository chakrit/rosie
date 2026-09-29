//! The lines the CLI prints about a scan besides the plan, from any mode. Every skip
//! names its path or subject and briefly why (`docs/spec/plan.md`).

use super::args::Mode;
use crate::plan::{Plan, PlanStats, Report, WalkSkips};
use crate::scan::{Refused, Scan, Skipped};

/// Every skip and problem of `scan`, one per line. `mode` decides whether a walk-skip
/// line may point to a flag: only the flags the mode that produced it actually accepts.
pub(super) fn findings_lines(scan: &Scan, mode: &Mode) -> Vec<String> {
    let skipped = scan
        .skipped
        .iter()
        .map(|skipped| walk_skip_line(skipped, mode));
    let refused = scan.refused.iter().map(refused_line);
    let reports = scan.plan.reports().iter().map(report_line);
    let problems = scan
        .problems
        .iter()
        .map(|problem| format!("problem: {problem}"));

    skipped
        .chain(refused)
        .chain(reports)
        .chain(problems)
        .collect()
}

/// The stats scan and confirmation show: the plan's tallies, then walk-skip counts.
pub(super) fn stats_lines(plan: &Plan, skips: WalkSkips) -> String {
    let stats = PlanStats::of(plan).to_string();
    match skips.total() {
        0 => stats,
        _ => format!("{stats}{skips}\n"),
    }
}

fn walk_skip_line(skipped: &Skipped, mode: &Mode) -> String {
    let path = skipped.path.display();
    let reason = skipped.reason.reason();
    match mode.walk_skip_hint(skipped.reason) {
        Some(hint) => format!("skipped {path}: {reason} ({hint})"),
        None => format!("skipped {path}: {reason}"),
    }
}

fn refused_line(refused: &Refused) -> String {
    format!(
        "skipped {} ({}): in use by process {} ({})",
        refused.path.display(),
        refused.size,
        refused.process.pid,
        refused.process.comm.display()
    )
}

fn report_line(report: &Report) -> String {
    format!(
        "skipped {}: rosie cannot remove it; by hand: {}",
        report.subject(),
        report.steps().join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Needs, WalkSkip};

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
        };

        assert_eq!(
            findings_lines(&scan, &Mode::default()),
            [
                "skipped login item Bar: rosie cannot remove it; by hand: Open System Settings > \
                 General > Login Items; Remove Bar"
            ]
        );
    }

    #[test]
    fn an_orphans_placeholder_skip_names_no_flag_the_mode_does_not_accept() {
        let scan = Scan {
            plan: Plan::default(),
            skipped: vec![Skipped {
                path: "/Users/x/Library/Caches/com.gone.Tool".into(),
                reason: WalkSkip::Closed(Needs::PLACEHOLDERS),
            }],
            refused: Vec::new(),
            problems: Vec::new(),
        };

        assert_eq!(
            findings_lines(&scan, &Mode::Orphans),
            ["skipped /Users/x/Library/Caches/com.gone.Tool: cloud placeholder, not downloaded"]
        );
    }

    #[test]
    fn a_tree_placeholder_skip_names_the_flag_that_mode_accepts() {
        let scan = Scan {
            plan: Plan::default(),
            skipped: vec![Skipped {
                path: "/Users/x/Library/Caches/com.gone.Tool".into(),
                reason: WalkSkip::Closed(Needs::PLACEHOLDERS),
            }],
            refused: Vec::new(),
            problems: Vec::new(),
        };

        assert_eq!(
            findings_lines(&scan, &Mode::default()),
            [
                "skipped /Users/x/Library/Caches/com.gone.Tool: cloud placeholder, not \
                 downloaded (use --enter-placeholders to open)"
            ]
        );
    }
}
