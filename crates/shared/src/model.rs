use serde::{Deserialize, Serialize};
use serde_with::{DurationNanoSeconds, serde_as};
use std::ops::{Add, AddAssign};
use std::time::Duration;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageDelta {
    pub click_count: u64,
    pub drag_duration: Duration,
    pub key_count: u64,
    pub left_modifier_duration: ModifierUsageDelta,
    pub right_modifier_duration: ModifierUsageDelta,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ModifierUsageDelta {
    pub shift: Duration,
    pub ctrl: Duration,
    pub alt: Duration,
    pub meta: Duration,
}

#[serde_as]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub click_count: u64,
    #[serde_as(as = "DurationNanoSeconds<u64>")]
    pub drag_duration: Duration,
    pub key_count: u64,
    pub left_modifier_duration: ModifierUsageSnapshot,
    pub right_modifier_duration: ModifierUsageSnapshot,
}

impl AddAssign<&UsageDelta> for UsageSnapshot {
    fn add_assign(&mut self, delta: &UsageDelta) {
        self.click_count += delta.click_count;
        self.drag_duration += delta.drag_duration;
        self.key_count += delta.key_count;
        self.left_modifier_duration += &delta.left_modifier_duration;
        self.right_modifier_duration += &delta.right_modifier_duration;
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
            key_count: self.key_count.saturating_sub(previous.key_count),
            left_modifier_duration: self
                .left_modifier_duration
                .saturating_delta(&previous.left_modifier_duration),
            right_modifier_duration: self
                .right_modifier_duration
                .saturating_delta(&previous.right_modifier_duration),
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
