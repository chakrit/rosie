//! What a scan found, from any mode, and the lines the CLI prints about it besides the
//! plan. Every skip names its path or subject and briefly why (`docs/spec/plan.md`).

use super::args::Mode;
use crate::apps::Scanned;
use crate::plan::{Plan, PlanStats, Report, WalkSkips};
use crate::scan::{Problem, Refused, Scan, Skipped};

pub(super) struct Findings {
    pub plan: Plan,
    pub skipped: Vec<Skipped>,
    pub refused: Vec<Refused>,
    pub problems: Vec<Problem>,
}

impl From<Scan> for Findings {
    fn from(scan: Scan) -> Self {
        Findings {
            plan: scan.plan,
            skipped: scan.skipped,
            refused: scan.refused,
            problems: scan.problems,
        }
    }
}

impl From<Scanned> for Findings {
    fn from(scanned: Scanned) -> Self {
        Findings {
            plan: scanned.plan,
            skipped: scanned.skipped,
            refused: Vec::new(),
            problems: Vec::new(),
        }
    }
}

impl Findings {
    pub fn skip_counts(&self) -> WalkSkips {
        self.skipped.iter().map(|skipped| skipped.reason).collect()
    }

    /// Whether the scan lost an item or a rule check to an error; rosie then exits
    /// non-zero.
    pub fn errored(&self) -> bool {
        !self.problems.is_empty()
    }

    /// Every skip and problem, one per line. `mode` decides whether a walk-skip line may
    /// point to a flag: only the flags the mode that produced it actually accepts.
    pub fn lines(&self, mode: &Mode) -> Vec<String> {
        let skipped = self
            .skipped
            .iter()
            .map(|skipped| walk_skip_line(skipped, mode));
        let refused = self.refused.iter().map(refused_line);
        let reports = self.plan.reports().iter().map(report_line);
        let problems = self
            .problems
            .iter()
            .map(|problem| format!("problem: {problem}"));

        skipped
            .chain(refused)
            .chain(reports)
            .chain(problems)
            .collect()
    }
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
        "skipped {}: in use by process {} ({})",
        refused.path.display(),
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

    #[test]
    fn a_report_only_item_is_a_skip_naming_its_subject_why_and_the_steps() {
        let plan = Plan::parse(
            "version = 1\n\n[[report]]\nsubject = \"login item Bar\"\nsteps = [\"Open System Settings > General > Login Items\", \"Remove Bar\"]\n",
        )
        .expect("valid plan");
        let findings = Findings {
            plan,
            skipped: Vec::new(),
            refused: Vec::new(),
            problems: Vec::new(),
        };

        assert_eq!(
            findings.lines(&Mode::default()),
            [
                "skipped login item Bar: rosie cannot remove it; by hand: Open System Settings > \
                 General > Login Items; Remove Bar"
            ]
        );
    }

    #[test]
    fn an_orphans_placeholder_skip_names_no_flag_the_mode_does_not_accept() {
        let findings = Findings {
            plan: Plan::default(),
            skipped: vec![Skipped {
                path: "/Users/x/Library/Caches/com.gone.Tool".into(),
                reason: crate::plan::WalkSkip::Placeholder,
            }],
            refused: Vec::new(),
            problems: Vec::new(),
        };

        assert_eq!(
            findings.lines(&Mode::Orphans),
            ["skipped /Users/x/Library/Caches/com.gone.Tool: cloud placeholder, not downloaded"]
        );
    }

    #[test]
    fn a_tree_placeholder_skip_names_the_flag_that_mode_accepts() {
        let findings = Findings {
            plan: Plan::default(),
            skipped: vec![Skipped {
                path: "/Users/x/Library/Caches/com.gone.Tool".into(),
                reason: crate::plan::WalkSkip::Placeholder,
            }],
            refused: Vec::new(),
            problems: Vec::new(),
        };

        assert_eq!(
            findings.lines(&Mode::default()),
            [
                "skipped /Users/x/Library/Caches/com.gone.Tool: cloud placeholder, not \
                 downloaded (use --enter-placeholders to open)"
            ]
        );
    }
}
