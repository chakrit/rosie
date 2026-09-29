//! Live scan progress: "found N · X GB" on stderr when it is a terminal, nothing when
//! it is not (`docs/spec/performance.md#scan`).

use std::io::{self, IsTerminal};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use indicatif::{ProgressBar, ProgressState, ProgressStyle};

use crate::plan::Size;

const TICK: Duration = Duration::from_millis(100);

/// Receives a scan's progress from every worker thread.
pub trait Progress: Sync {
    /// A target was found.
    fn found(&self);

    /// Sizing counted `bytes` more allocated bytes.
    fn sized(&self, bytes: u64);
}

/// Shows nothing.
pub struct NoProgress;

impl Progress for NoProgress {
    fn found(&self) {}

    fn sized(&self, _bytes: u64) {}
}

/// A spinner with "found N · X GB" on a terminal stderr; nothing otherwise.
pub struct StderrProgress {
    counts: Arc<Counts>,
    spinner: Option<ProgressBar>,
}

#[derive(Default)]
struct Counts {
    found: AtomicUsize,
    bytes: AtomicU64,
}

impl Counts {
    /// `found 3 · 1.2 GB`.
    fn summary(&self) -> String {
        let found = self.found.load(Ordering::Relaxed);
        let bytes = Size::bytes(self.bytes.load(Ordering::Relaxed));
        format!("found {found} · {bytes}")
    }
}

impl StderrProgress {
    pub fn new() -> Self {
        let counts = Arc::new(Counts::default());
        let spinner = io::stderr()
            .is_terminal()
            .then(|| spinner(Arc::clone(&counts)));
        StderrProgress { counts, spinner }
    }

    /// Clears the spinner.
    pub fn finish(&self) {
        if let Some(spinner) = &self.spinner {
            spinner.finish_and_clear();
        }
    }
}

impl Default for StderrProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl Progress for StderrProgress {
    fn found(&self) {
        self.counts.found.fetch_add(1, Ordering::Relaxed);
    }

    fn sized(&self, bytes: u64) {
        // Plan sizes saturate (`Measure::add`); the displayed total must agree with
        // them instead of wrapping past `u64::MAX`.
        let saturating = |current: u64| Some(current.saturating_add(bytes));
        let updated =
            self.counts
                .bytes
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, saturating);
        if let Err(total) = updated {
            unreachable!("a saturating add always yields a total, yet {total} was kept");
        }
    }
}

/// The spinner reads the counts on each tick, so workers only bump atomics.
fn spinner(counts: Arc<Counts>) -> ProgressBar {
    let style = ProgressStyle::with_template("{spinner} {summary}")
        .unwrap_or_else(|error| unreachable!("the spinner template is fixed and valid: {error}"))
        .with_key(
            "summary",
            move |_: &ProgressState, out: &mut dyn std::fmt::Write| {
                // indicatif gives a key no way to report a failed write; it would only
                // lose one frame of the spinner.
                let _ = out.write_str(&counts.summary());
            },
        );
    let spinner = ProgressBar::new_spinner().with_style(style);
    spinner.enable_steady_tick(TICK);
    spinner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_the_found_count_and_the_size_so_far() {
        let progress = StderrProgress {
            counts: Arc::default(),
            spinner: None,
        };
        let before = progress.counts.summary();

        progress.found();
        progress.sized(1_000_000_000);
        progress.found();
        progress.found();
        progress.sized(234_000_000);

        assert_eq!(before, "found 0 · 0 B");
        assert_eq!(progress.counts.summary(), "found 3 · 1.2 GB");
    }

    #[test]
    fn the_byte_total_saturates_instead_of_wrapping() {
        let progress = StderrProgress {
            counts: Arc::default(),
            spinner: None,
        };

        progress.sized(u64::MAX);
        progress.sized(u64::MAX);

        assert_eq!(progress.counts.bytes.load(Ordering::Relaxed), u64::MAX);
    }
}
