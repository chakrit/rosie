//! Scan progress, reported from every worker thread to whatever shows it
//! (`docs/spec/performance.md#scan`).

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
