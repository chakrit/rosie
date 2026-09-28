use std::fmt;
use std::ops::{Add, AddAssign};

/// Allocated bytes on disk, with hardlinks counted once (`docs/spec/plan.md#stats`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Size(u64);

const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];

impl Size {
    pub const ZERO: Size = Size(0);

    pub const fn bytes(bytes: u64) -> Self {
        Size(bytes)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl Add for Size {
    type Output = Size;

    fn add(self, other: Size) -> Size {
        Size(self.0.saturating_add(other.0))
    }
}

impl AddAssign for Size {
    fn add_assign(&mut self, other: Size) {
        *self = *self + other;
    }
}

/// Decimal units, as Finder shows them: `512 B`, `1.5 KB`, `3.4 GB`.
impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 < 1000 {
            return write!(f, "{} B", self.0);
        }

        let mut value = self.0 as f64 / 1000.0;
        let mut unit = 0;
        while value >= 999.95 && unit + 1 < UNITS.len() {
            value /= 1000.0;
            unit += 1;
        }
        write!(f, "{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_decimal_units_with_one_decimal() {
        let shown = |bytes| Size::bytes(bytes).to_string();

        assert_eq!(shown(0), "0 B");
        assert_eq!(shown(999), "999 B");
        assert_eq!(shown(1000), "1.0 KB");
        assert_eq!(shown(1_536_000), "1.5 MB");
        assert_eq!(shown(999_960), "1.0 MB");
        assert_eq!(shown(3_400_000_000), "3.4 GB");
    }
}
