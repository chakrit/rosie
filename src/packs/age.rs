//! Pack age: the time since a pack's pull (`docs/spec/rules.md#packs`).

use std::fmt;
use std::time::{Duration, SystemTime};

use super::store::Installed;

/// One month: the average Gregorian month, 365.2425 / 12 days.
pub const MONTH: Duration = Duration::from_secs(2_629_746);

const STALE_MONTHS: u64 = 6;

/// The warning shown once the rules are 6 months old.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgeWarning {
    pub months: u64,
}

impl fmt::Display for AgeWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Your rules are {} months old, consider rosie rules pull to update",
            self.months
        )
    }
}

/// The age warning for the oldest installed pack, when it is 6 months old or more.
pub fn age_warning(installed: &[Installed], now: SystemTime) -> Option<AgeWarning> {
    let oldest = installed.iter().map(|pack| pack.pulled_at).min()?;
    // A pull time ahead of the clock is not stale.
    let age = now.duration_since(oldest).unwrap_or_default();

    let months = age.as_secs() / MONTH.as_secs();
    match months >= STALE_MONTHS {
        true => Some(AgeWarning { months }),
        false => None,
    }
}
