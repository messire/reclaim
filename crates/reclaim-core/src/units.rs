//! Byte quantities and their display (ARCHITECTURE §5.1).

use std::fmt;
use std::ops::{Add, AddAssign};

use serde::{Deserialize, Serialize};

/// An amount of memory in bytes. Display uses IEC units (KiB, MiB, GiB).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Bytes(pub u64);

impl Bytes {
    /// Zero bytes.
    pub const ZERO: Bytes = Bytes(0);

    /// Creates a value from mebibytes.
    pub const fn from_mib(mib: u64) -> Bytes {
        Bytes(mib * 1024 * 1024)
    }

    /// The raw byte count.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// `self - other`, clamped at zero.
    pub const fn saturating_sub(self, other: Bytes) -> Bytes {
        Bytes(self.0.saturating_sub(other.0))
    }

    /// Signed difference `self - earlier`.
    pub fn delta_from(self, earlier: Bytes) -> ByteDelta {
        ByteDelta(i128::from(self.0) - i128::from(earlier.0))
    }

    /// Value in mebibytes as a float, for display and ratios.
    pub fn as_mib_f64(self) -> f64 {
        self.0 as f64 / (1024.0 * 1024.0)
    }
}

impl Add for Bytes {
    type Output = Bytes;
    fn add(self, rhs: Bytes) -> Bytes {
        Bytes(self.0.saturating_add(rhs.0))
    }
}

impl AddAssign for Bytes {
    fn add_assign(&mut self, rhs: Bytes) {
        *self = *self + rhs;
    }
}

impl std::iter::Sum for Bytes {
    fn sum<I: Iterator<Item = Bytes>>(iter: I) -> Bytes {
        iter.fold(Bytes::ZERO, |a, b| a + b)
    }
}

const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

fn format_magnitude(f: &mut fmt::Formatter<'_>, value: u128) -> fmt::Result {
    if value < 1024 {
        return write!(f, "{value} B");
    }
    let mut v = value as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    // One decimal below 100, none above: "3.4 GiB", "512 MiB".
    if v < 100.0 {
        write!(f, "{v:.1} {}", UNITS[unit])
    } else {
        write!(f, "{v:.0} {}", UNITS[unit])
    }
}

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format_magnitude(f, u128::from(self.0))
    }
}

/// A signed difference between two byte amounts.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ByteDelta(pub i128);

impl ByteDelta {
    /// No change.
    pub const ZERO: ByteDelta = ByteDelta(0);

    /// Magnitude of the change.
    pub fn abs(self) -> Bytes {
        Bytes(u64::try_from(self.0.unsigned_abs()).unwrap_or(u64::MAX))
    }

    /// True if the value went down.
    pub fn is_decrease(self) -> bool {
        self.0 < 0
    }
}

impl fmt::Display for ByteDelta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = match self.0.cmp(&0) {
            std::cmp::Ordering::Less => "−",
            std::cmp::Ordering::Equal => "±",
            std::cmp::Ordering::Greater => "+",
        };
        f.write_str(sign)?;
        format_magnitude(f, self.0.unsigned_abs())
    }
}

/// A count of page faults or I/O operations.
pub type Count = u64;

/// A duration in milliseconds, used for timestamps in records (portable and serializable).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Millis(pub u64);

impl Millis {
    /// Elapsed time from `earlier` to `self`, clamped at zero.
    pub const fn since(self, earlier: Millis) -> Millis {
        Millis(self.0.saturating_sub(earlier.0))
    }

    /// Adds a duration in milliseconds.
    pub const fn plus(self, ms: u64) -> Millis {
        Millis(self.0.saturating_add(ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_iec_units() {
        assert_eq!(Bytes(0).to_string(), "0 B");
        assert_eq!(Bytes(1023).to_string(), "1023 B");
        assert_eq!(Bytes(1024).to_string(), "1.0 KiB");
        assert_eq!(Bytes::from_mib(512).to_string(), "512 MiB");
        assert_eq!(Bytes::from_mib(1536).to_string(), "1.5 GiB");
        assert_eq!(Bytes(u64::MAX).to_string(), "16.0 EiB");
    }

    #[test]
    fn deltas_are_signed() {
        let d = Bytes::from_mib(100).delta_from(Bytes::from_mib(300));
        assert!(d.is_decrease());
        assert_eq!(d.abs(), Bytes::from_mib(200));
        assert_eq!(d.to_string(), "−200 MiB");
        assert_eq!(Bytes(5).delta_from(Bytes(5)).to_string(), "±0 B");
        assert_eq!(Bytes(2048).delta_from(Bytes(0)).to_string(), "+2.0 KiB");
    }

    #[test]
    fn arithmetic_saturates() {
        assert_eq!(Bytes(u64::MAX) + Bytes(1), Bytes(u64::MAX));
        assert_eq!(Bytes(1).saturating_sub(Bytes(2)), Bytes::ZERO);
        let total: Bytes = [Bytes(1), Bytes(2), Bytes(3)].into_iter().sum();
        assert_eq!(total, Bytes(6));
        let mut b = Bytes(1);
        b += Bytes(1);
        assert_eq!(b.get(), 2);
        assert!((Bytes::from_mib(3).as_mib_f64() - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn millis() {
        assert_eq!(Millis(10).since(Millis(4)), Millis(6));
        assert_eq!(Millis(4).since(Millis(10)), Millis(0));
        assert_eq!(Millis(4).plus(6), Millis(10));
    }
}
