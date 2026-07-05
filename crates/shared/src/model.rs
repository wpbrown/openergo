use serde::{Deserialize, Serialize};
use serde_with::{DurationNanoSeconds, serde_as};
use std::ops::{Add, AddAssign, Mul, MulAssign};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyCount {
    pub left: u64,
    pub right: u64,
    pub other: u64,
}

impl KeyCount {
    pub fn total(self) -> u64 {
        self.left + self.right + self.other
    }

    pub fn saturating_delta(self, previous: Self) -> Self {
        Self {
            left: self.left.saturating_sub(previous.left),
            right: self.right.saturating_sub(previous.right),
            other: self.other.saturating_sub(previous.other),
        }
    }
}

impl AddAssign<KeyCount> for KeyCount {
    fn add_assign(&mut self, delta: KeyCount) {
        self.left += delta.left;
        self.right += delta.right;
        self.other += delta.other;
    }
}

impl Add<KeyCount> for KeyCount {
    type Output = Self;

    fn add(mut self, delta: KeyCount) -> Self {
        self += delta;
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageDelta {
    pub click_count: u64,
    pub drag_duration: Duration,
    pub key_count: KeyCount,
    pub scroll_count: u64,
    pub left_modifier_duration: ModifierUsageDelta,
    pub right_modifier_duration: ModifierUsageDelta,
    /// Time the user was generating usage-tracked input during this delta.
    pub active_duration: Duration,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ModifierUsageDelta {
    pub shift: Duration,
    pub ctrl: Duration,
    pub alt: Duration,
    pub meta: Duration,
}

impl AddAssign<&UsageDelta> for UsageDelta {
    fn add_assign(&mut self, delta: &UsageDelta) {
        self.click_count += delta.click_count;
        self.drag_duration += delta.drag_duration;
        self.key_count += delta.key_count;
        self.scroll_count += delta.scroll_count;
        self.left_modifier_duration += &delta.left_modifier_duration;
        self.right_modifier_duration += &delta.right_modifier_duration;
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

impl AddAssign<&ModifierUsageDelta> for ModifierUsageDelta {
    fn add_assign(&mut self, delta: &ModifierUsageDelta) {
        self.shift += delta.shift;
        self.ctrl += delta.ctrl;
        self.alt += delta.alt;
        self.meta += delta.meta;
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
    pub click_count: u64,
    #[serde_as(as = "DurationNanoSeconds<u64>")]
    pub drag_duration: Duration,
    pub key_count: KeyCount,
    pub scroll_count: u64,
    pub left_modifier_duration: ModifierUsageSnapshot,
    pub right_modifier_duration: ModifierUsageSnapshot,
    #[serde_as(as = "DurationNanoSeconds<u64>")]
    pub active_duration: Duration,
}

impl AddAssign<&UsageDelta> for UsageSnapshot {
    fn add_assign(&mut self, delta: &UsageDelta) {
        self.click_count += delta.click_count;
        self.drag_duration += delta.drag_duration;
        self.key_count += delta.key_count;
        self.scroll_count += delta.scroll_count;
        self.left_modifier_duration += &delta.left_modifier_duration;
        self.right_modifier_duration += &delta.right_modifier_duration;
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
            click_count: self.click_count.saturating_sub(previous.click_count),
            drag_duration: self.drag_duration.saturating_sub(previous.drag_duration),
            key_count: self.key_count.saturating_delta(previous.key_count),
            scroll_count: self.scroll_count.saturating_sub(previous.scroll_count),
            left_modifier_duration: self
                .left_modifier_duration
                .saturating_delta(&previous.left_modifier_duration),
            right_modifier_duration: self
                .right_modifier_duration
                .saturating_delta(&previous.right_modifier_duration),
            active_duration: self
                .active_duration
                .saturating_sub(previous.active_duration),
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
}

impl AddAssign<&ModifierUsageDelta> for ModifierUsageSnapshot {
    fn add_assign(&mut self, delta: &ModifierUsageDelta) {
        self.shift += delta.shift;
        self.ctrl += delta.ctrl;
        self.alt += delta.alt;
        self.meta += delta.meta;
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
    fn ratio_divides_credit_by_limit_without_rounding() {
        assert_eq!(ratio(Credit::new(1.0), CreditLimit::new(3.0)), 1.0 / 3.0);
    }

    #[test]
    fn ratio_returns_zero_for_non_positive_limit() {
        assert_eq!(ratio(Credit::new(1.0), CreditLimit::ZERO), 0.0);
        assert_eq!(ratio(Credit::new(1.0), CreditLimit::new(-1.0)), 0.0);
    }
}
