use serde::{Deserialize, Serialize};
use serde_with::{DurationNanoSeconds, serde_as};
use std::ops::{Add, AddAssign, Mul, MulAssign};
use std::time::Duration;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageDelta {
    pub left: HandUsageDelta,
    pub right: HandUsageDelta,
    pub unclassified_key_count: u64,
    pub unclassified_key_combo: u64,
    /// Time the user was generating usage-tracked input during this delta.
    pub active_duration: Duration,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct HandUsageDelta {
    pub click_count: u64,
    pub drag_duration: Duration,
    pub key_count: u64,
    pub scroll_count: u64,
    pub modifier: ModifierUsageDelta,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ModifierUsageDelta {
    pub shift: Duration,
    pub ctrl: Duration,
    pub alt: Duration,
    pub meta: Duration,
    pub multi: Duration,
    pub same_hand_combo: u64,
}

impl AddAssign<&UsageDelta> for UsageDelta {
    fn add_assign(&mut self, delta: &UsageDelta) {
        self.left += &delta.left;
        self.right += &delta.right;
        self.unclassified_key_count += delta.unclassified_key_count;
        self.unclassified_key_combo += delta.unclassified_key_combo;
        self.active_duration += delta.active_duration;
    }
}

impl Add<&UsageDelta> for UsageDelta {
    type Output = Self;

    fn add(mut self, delta: &UsageDelta) -> Self {
        self += delta;
        self
    }
}

impl UsageDelta {
    /// Count every non-modifier key event represented by this delta.
    ///
    /// Key and combo buckets are mutually exclusive on the server, so callers
    /// that need the physical keyboard-event aggregate should use this helper.
    pub fn key_event_count(&self) -> u64 {
        self.left
            .key_count
            .saturating_add(self.right.key_count)
            .saturating_add(self.left.modifier.same_hand_combo)
            .saturating_add(self.right.modifier.same_hand_combo)
            .saturating_add(self.unclassified_key_count)
            .saturating_add(self.unclassified_key_combo)
    }
}

impl AddAssign<&HandUsageDelta> for HandUsageDelta {
    fn add_assign(&mut self, delta: &HandUsageDelta) {
        self.click_count += delta.click_count;
        self.drag_duration += delta.drag_duration;
        self.key_count += delta.key_count;
        self.scroll_count += delta.scroll_count;
        self.modifier += &delta.modifier;
    }
}

impl Add<&HandUsageDelta> for HandUsageDelta {
    type Output = Self;

    fn add(mut self, delta: &HandUsageDelta) -> Self {
        self += delta;
        self
    }
}

impl AddAssign<&ModifierUsageDelta> for ModifierUsageDelta {
    fn add_assign(&mut self, delta: &ModifierUsageDelta) {
        self.shift += delta.shift;
        self.ctrl += delta.ctrl;
        self.alt += delta.alt;
        self.meta += delta.meta;
        self.multi += delta.multi;
        self.same_hand_combo += delta.same_hand_combo;
    }
}

impl Add<&ModifierUsageDelta> for ModifierUsageDelta {
    type Output = Self;

    fn add(mut self, delta: &ModifierUsageDelta) -> Self {
        self += delta;
        self
    }
}

#[serde_as]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub left: HandUsageSnapshot,
    pub right: HandUsageSnapshot,
    pub unclassified_key_count: u64,
    pub unclassified_key_combo: u64,
    #[serde_as(as = "DurationNanoSeconds<u64>")]
    pub active_duration: Duration,
}

impl AddAssign<&UsageDelta> for UsageSnapshot {
    fn add_assign(&mut self, delta: &UsageDelta) {
        self.left += &delta.left;
        self.right += &delta.right;
        self.unclassified_key_count += delta.unclassified_key_count;
        self.unclassified_key_combo += delta.unclassified_key_combo;
        self.active_duration += delta.active_duration;
    }
}

impl Add<&UsageDelta> for UsageSnapshot {
    type Output = Self;

    fn add(mut self, delta: &UsageDelta) -> Self {
        self += delta;
        self
    }
}

impl UsageSnapshot {
    pub fn saturating_delta(&self, previous: &UsageSnapshot) -> UsageDelta {
        UsageDelta {
            left: self.left.saturating_delta(&previous.left),
            right: self.right.saturating_delta(&previous.right),
            unclassified_key_count: self
                .unclassified_key_count
                .saturating_sub(previous.unclassified_key_count),
            unclassified_key_combo: self
                .unclassified_key_combo
                .saturating_sub(previous.unclassified_key_combo),
            active_duration: self
                .active_duration
                .saturating_sub(previous.active_duration),
        }
    }
}

#[serde_as]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct HandUsageSnapshot {
    pub click_count: u64,
    #[serde_as(as = "DurationNanoSeconds<u64>")]
    pub drag_duration: Duration,
    pub key_count: u64,
    pub scroll_count: u64,
    pub modifier: ModifierUsageSnapshot,
}

impl AddAssign<&HandUsageDelta> for HandUsageSnapshot {
    fn add_assign(&mut self, delta: &HandUsageDelta) {
        self.click_count += delta.click_count;
        self.drag_duration += delta.drag_duration;
        self.key_count += delta.key_count;
        self.scroll_count += delta.scroll_count;
        self.modifier += &delta.modifier;
    }
}

impl Add<&HandUsageDelta> for HandUsageSnapshot {
    type Output = Self;

    fn add(mut self, delta: &HandUsageDelta) -> Self {
        self += delta;
        self
    }
}

impl HandUsageSnapshot {
    pub fn saturating_delta(&self, previous: &HandUsageSnapshot) -> HandUsageDelta {
        HandUsageDelta {
            click_count: self.click_count.saturating_sub(previous.click_count),
            drag_duration: self.drag_duration.saturating_sub(previous.drag_duration),
            key_count: self.key_count.saturating_sub(previous.key_count),
            scroll_count: self.scroll_count.saturating_sub(previous.scroll_count),
            modifier: self.modifier.saturating_delta(&previous.modifier),
        }
    }
}

#[serde_as]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ModifierUsageSnapshot {
    #[serde_as(as = "DurationNanoSeconds<u64>")]
    pub shift: Duration,
    #[serde_as(as = "DurationNanoSeconds<u64>")]
    pub ctrl: Duration,
    #[serde_as(as = "DurationNanoSeconds<u64>")]
    pub alt: Duration,
    #[serde_as(as = "DurationNanoSeconds<u64>")]
    pub meta: Duration,
    /// Union time while more than one modifier was held on this hand.
    #[serde_as(as = "DurationNanoSeconds<u64>")]
    pub multi: Duration,
    /// Same-hand non-modifier key presses while this hand held a modifier.
    pub same_hand_combo: u64,
}

impl AddAssign<&ModifierUsageDelta> for ModifierUsageSnapshot {
    fn add_assign(&mut self, delta: &ModifierUsageDelta) {
        self.shift += delta.shift;
        self.ctrl += delta.ctrl;
        self.alt += delta.alt;
        self.meta += delta.meta;
        self.multi += delta.multi;
        self.same_hand_combo += delta.same_hand_combo;
    }
}

impl Add<&ModifierUsageDelta> for ModifierUsageSnapshot {
    type Output = Self;

    fn add(mut self, delta: &ModifierUsageDelta) -> Self {
        self += delta;
        self
    }
}

impl ModifierUsageSnapshot {
    pub fn saturating_delta(&self, previous: &ModifierUsageSnapshot) -> ModifierUsageDelta {
        ModifierUsageDelta {
            shift: self.shift.saturating_sub(previous.shift),
            ctrl: self.ctrl.saturating_sub(previous.ctrl),
            alt: self.alt.saturating_sub(previous.alt),
            meta: self.meta.saturating_sub(previous.meta),
            multi: self.multi.saturating_sub(previous.multi),
            same_hand_combo: self
                .same_hand_combo
                .saturating_sub(previous.same_hand_combo),
        }
    }
}

/// Newtype around [`f64`] for accumulated credit.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Credit(f64);

impl Credit {
    pub const ZERO: Credit = Credit::new(0.0);

    pub const fn new(value: f64) -> Self {
        Self(value)
    }

    pub fn as_f64(self) -> f64 {
        self.0
    }

    pub fn saturating_sub_zero(self, rhs: Credit) -> Credit {
        Credit((self.0 - rhs.0).max(0.0))
    }
}

impl Add for Credit {
    type Output = Credit;

    fn add(self, rhs: Credit) -> Credit {
        Credit(self.0 + rhs.0)
    }
}

impl AddAssign for Credit {
    fn add_assign(&mut self, rhs: Credit) {
        self.0 += rhs.0;
    }
}

impl Mul<f64> for Credit {
    type Output = Credit;

    fn mul(self, rhs: f64) -> Credit {
        Credit(self.0 * rhs)
    }
}

impl Mul for Credit {
    type Output = Credit;

    fn mul(self, rhs: Credit) -> Credit {
        Credit(self.0 * rhs.0)
    }
}

impl MulAssign<f64> for Credit {
    fn mul_assign(&mut self, rhs: f64) {
        self.0 *= rhs;
    }
}

/// Newtype around [`f64`] for a credit budget / limit. Distinct from
/// [`Credit`] (accumulated total) to avoid mixing the two at call sites.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CreditLimit(pub f64);

impl CreditLimit {
    pub const ZERO: CreditLimit = CreditLimit(0.0);

    pub const fn new(value: f64) -> Self {
        Self(value)
    }

    pub fn as_f64(self) -> f64 {
        self.0
    }
}

pub fn ratio(credit: Credit, limit: CreditLimit) -> f64 {
    if limit > CreditLimit::ZERO {
        credit.as_f64() / limit.as_f64()
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_snapshot_accumulates_and_diffs_handed_usage_fields() {
        let previous = UsageSnapshot {
            left: HandUsageSnapshot {
                click_count: 3,
                modifier: ModifierUsageSnapshot {
                    multi: Duration::from_millis(20),
                    same_hand_combo: 7,
                    ..ModifierUsageSnapshot::default()
                },
                ..HandUsageSnapshot::default()
            },
            right: HandUsageSnapshot {
                scroll_count: 5,
                ..HandUsageSnapshot::default()
            },
            unclassified_key_count: 11,
            unclassified_key_combo: 13,
            ..UsageSnapshot::default()
        };
        let mut snapshot = previous.clone();

        snapshot += &UsageDelta {
            left: HandUsageDelta {
                click_count: 2,
                modifier: ModifierUsageDelta {
                    multi: Duration::from_millis(30),
                    same_hand_combo: 17,
                    ..ModifierUsageDelta::default()
                },
                ..HandUsageDelta::default()
            },
            right: HandUsageDelta {
                scroll_count: 4,
                ..HandUsageDelta::default()
            },
            unclassified_key_count: 19,
            unclassified_key_combo: 23,
            ..UsageDelta::default()
        };

        let delta = snapshot.saturating_delta(&previous);
        assert_eq!(delta.left.click_count, 2);
        assert_eq!(delta.right.scroll_count, 4);
        assert_eq!(delta.unclassified_key_count, 19);
        assert_eq!(delta.unclassified_key_combo, 23);
        assert_eq!(delta.left.modifier.multi, Duration::from_millis(30));
        assert_eq!(delta.left.modifier.same_hand_combo, 17);
    }

    #[test]
    fn key_event_count_includes_plain_combo_and_unclassified_buckets() {
        let delta = UsageDelta {
            left: HandUsageDelta {
                key_count: 2,
                modifier: ModifierUsageDelta {
                    same_hand_combo: 3,
                    ..ModifierUsageDelta::default()
                },
                ..HandUsageDelta::default()
            },
            right: HandUsageDelta {
                key_count: 5,
                modifier: ModifierUsageDelta {
                    same_hand_combo: 7,
                    ..ModifierUsageDelta::default()
                },
                ..HandUsageDelta::default()
            },
            unclassified_key_count: 11,
            unclassified_key_combo: 13,
            ..UsageDelta::default()
        };

        assert_eq!(delta.key_event_count(), 41);
    }

    #[test]
    fn snapshot_delta_saturates_handed_usage_fields() {
        let previous = UsageSnapshot {
            left: HandUsageSnapshot {
                key_count: 10,
                modifier: ModifierUsageSnapshot {
                    same_hand_combo: 5,
                    shift: Duration::from_secs(3),
                    ..ModifierUsageSnapshot::default()
                },
                ..HandUsageSnapshot::default()
            },
            unclassified_key_count: 8,
            ..UsageSnapshot::default()
        };
        let snapshot = UsageSnapshot {
            left: HandUsageSnapshot {
                key_count: 4,
                modifier: ModifierUsageSnapshot {
                    same_hand_combo: 2,
                    shift: Duration::from_secs(1),
                    ..ModifierUsageSnapshot::default()
                },
                ..HandUsageSnapshot::default()
            },
            unclassified_key_count: 3,
            ..UsageSnapshot::default()
        };

        let delta = snapshot.saturating_delta(&previous);
        assert_eq!(delta.left.key_count, 0);
        assert_eq!(delta.left.modifier.same_hand_combo, 0);
        assert_eq!(delta.left.modifier.shift, Duration::ZERO);
        assert_eq!(delta.unclassified_key_count, 0);
    }

    #[test]
    fn modifier_usage_accumulates_same_hand_combo() {
        let mut snapshot = ModifierUsageSnapshot {
            same_hand_combo: 2,
            shift: Duration::from_millis(10),
            ..ModifierUsageSnapshot::default()
        };

        snapshot += &ModifierUsageDelta {
            same_hand_combo: 3,
            shift: Duration::from_millis(20),
            ..ModifierUsageDelta::default()
        };

        assert_eq!(snapshot.same_hand_combo, 5);
        assert_eq!(snapshot.shift, Duration::from_millis(30));
    }

    #[test]
    fn hand_usage_snapshot_accumulates_and_diffs_modifier_usage() {
        let previous = HandUsageSnapshot {
            drag_duration: Duration::from_millis(10),
            modifier: ModifierUsageSnapshot {
                multi: Duration::from_millis(20),
                same_hand_combo: 7,
                ..ModifierUsageSnapshot::default()
            },
            ..HandUsageSnapshot::default()
        };
        let mut snapshot = previous;

        snapshot += &HandUsageDelta {
            drag_duration: Duration::from_millis(11),
            modifier: ModifierUsageDelta {
                multi: Duration::from_millis(30),
                same_hand_combo: 13,
                ..ModifierUsageDelta::default()
            },
            ..HandUsageDelta::default()
        };

        let delta = snapshot.saturating_delta(&previous);
        assert_eq!(delta.drag_duration, Duration::from_millis(11));
        assert_eq!(delta.modifier.multi, Duration::from_millis(30));
        assert_eq!(delta.modifier.same_hand_combo, 13);
    }

    #[test]
    fn ratio_divides_credit_by_limit_without_rounding() {
        assert_eq!(ratio(Credit::new(1.0), CreditLimit::new(3.0)), 1.0 / 3.0);
    }

    #[test]
    fn ratio_returns_zero_for_non_positive_limit() {
        assert_eq!(ratio(Credit::new(1.0), CreditLimit::ZERO), 0.0);
        assert_eq!(ratio(Credit::new(1.0), CreditLimit::new(-1.0)), 0.0);
    }
}
