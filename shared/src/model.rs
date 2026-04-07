use crate::protocol::{ModifierUsageIncrement, UsageIncrement};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_with::{DurationNanoSeconds, serde_as};
use std::time::Duration;

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

impl UsageSnapshot {
    pub fn saturating_usage_since(
        &self,
        previous: &UsageSnapshot,
        start: Timestamp,
        end: Timestamp,
    ) -> UsageIncrement {
        UsageIncrement {
            click_count: self.click_count.saturating_sub(previous.click_count),
            drag_duration: self.drag_duration.saturating_sub(previous.drag_duration),
            key_count: self.key_count.saturating_sub(previous.key_count),
            left_modifier_duration: self
                .left_modifier_duration
                .saturating_usage_since(&previous.left_modifier_duration),
            right_modifier_duration: self
                .right_modifier_duration
                .saturating_usage_since(&previous.right_modifier_duration),
            start,
            end,
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

impl ModifierUsageSnapshot {
    pub fn saturating_usage_since(
        &self,
        previous: &ModifierUsageSnapshot,
    ) -> ModifierUsageIncrement {
        ModifierUsageIncrement {
            shift: self.shift.saturating_sub(previous.shift),
            ctrl: self.ctrl.saturating_sub(previous.ctrl),
            alt: self.alt.saturating_sub(previous.alt),
            meta: self.meta.saturating_sub(previous.meta),
        }
    }
}
