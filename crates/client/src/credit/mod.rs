pub mod calculator;
pub mod limit;
pub mod utilization;

pub use calculator::CreditCalculator;
pub use calculator::config::CreditCalculatorConfig;
use serde::{Deserialize, Serialize};
pub use shared::model::Credit;
use std::ops::{Add, AddAssign};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ModifierCreditDelta {
    pub shift: Credit,
    pub ctrl: Credit,
    pub alt: Credit,
    pub meta: Credit,
}

impl AddAssign<&ModifierCreditDelta> for ModifierCreditDelta {
    fn add_assign(&mut self, delta: &ModifierCreditDelta) {
        self.shift += delta.shift;
        self.ctrl += delta.ctrl;
        self.alt += delta.alt;
        self.meta += delta.meta;
    }
}

impl Add<&ModifierCreditDelta> for ModifierCreditDelta {
    type Output = Self;

    fn add(mut self, delta: &ModifierCreditDelta) -> Self {
        self += delta;
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreditDelta {
    pub click: Credit,
    pub drag: Credit,
    pub key: Credit,
    pub scroll: Credit,
    pub left_modifier: ModifierCreditDelta,
    pub right_modifier: ModifierCreditDelta,
}

impl AddAssign<&CreditDelta> for CreditDelta {
    fn add_assign(&mut self, delta: &CreditDelta) {
        self.click += delta.click;
        self.drag += delta.drag;
        self.key += delta.key;
        self.scroll += delta.scroll;
        self.left_modifier += &delta.left_modifier;
        self.right_modifier += &delta.right_modifier;
    }
}

impl Add<&CreditDelta> for CreditDelta {
    type Output = Self;

    fn add(mut self, delta: &CreditDelta) -> Self {
        self += delta;
        self
    }
}

impl ModifierCreditDelta {
    pub fn total(&self) -> Credit {
        self.shift + self.ctrl + self.alt + self.meta
    }

    pub fn scaled(&self, multiplier: f64) -> Self {
        Self {
            shift: self.shift * multiplier,
            ctrl: self.ctrl * multiplier,
            alt: self.alt * multiplier,
            meta: self.meta * multiplier,
        }
    }
}

impl CreditDelta {
    pub fn total(&self) -> Credit {
        self.click
            + self.drag
            + self.key
            + self.scroll
            + self.left_modifier.total()
            + self.right_modifier.total()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreditIncrement {
    pub base: CreditDelta,
    pub boost: CreditDelta,
}

impl AddAssign<&CreditIncrement> for CreditIncrement {
    fn add_assign(&mut self, increment: &CreditIncrement) {
        self.base += &increment.base;
        self.boost += &increment.boost;
    }
}

impl Add<&CreditIncrement> for CreditIncrement {
    type Output = Self;

    fn add(mut self, increment: &CreditIncrement) -> Self {
        self += increment;
        self
    }
}

impl CreditIncrement {
    pub fn total(&self) -> Credit {
        self.base.total() + self.boost.total()
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ModifierCreditSnapshot {
    pub shift: Credit,
    pub ctrl: Credit,
    pub alt: Credit,
    pub meta: Credit,
}

impl AddAssign<&ModifierCreditDelta> for ModifierCreditSnapshot {
    fn add_assign(&mut self, delta: &ModifierCreditDelta) {
        self.shift += delta.shift;
        self.ctrl += delta.ctrl;
        self.alt += delta.alt;
        self.meta += delta.meta;
    }
}

impl Add<&ModifierCreditDelta> for ModifierCreditSnapshot {
    type Output = Self;

    fn add(mut self, delta: &ModifierCreditDelta) -> Self {
        self += delta;
        self
    }
}

impl ModifierCreditSnapshot {
    pub fn saturating_delta(&self, previous: &ModifierCreditSnapshot) -> ModifierCreditDelta {
        ModifierCreditDelta {
            shift: self.shift.saturating_sub_zero(previous.shift),
            ctrl: self.ctrl.saturating_sub_zero(previous.ctrl),
            alt: self.alt.saturating_sub_zero(previous.alt),
            meta: self.meta.saturating_sub_zero(previous.meta),
        }
    }

    /// Sum of all four modifier credit fields.
    pub fn total(&self) -> Credit {
        self.shift + self.ctrl + self.alt + self.meta
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreditSnapshot {
    pub click: Credit,
    pub drag: Credit,
    pub key: Credit,
    pub scroll: Credit,
    pub left_modifier: ModifierCreditSnapshot,
    pub right_modifier: ModifierCreditSnapshot,
}

impl AddAssign<&CreditDelta> for CreditSnapshot {
    fn add_assign(&mut self, delta: &CreditDelta) {
        self.click += delta.click;
        self.drag += delta.drag;
        self.key += delta.key;
        self.scroll += delta.scroll;
        self.left_modifier += &delta.left_modifier;
        self.right_modifier += &delta.right_modifier;
    }
}

impl Add<&CreditDelta> for CreditSnapshot {
    type Output = Self;

    fn add(mut self, delta: &CreditDelta) -> Self {
        self += delta;
        self
    }
}

impl CreditSnapshot {
    pub fn saturating_delta(&self, previous: &CreditSnapshot) -> CreditDelta {
        CreditDelta {
            click: self.click.saturating_sub_zero(previous.click),
            drag: self.drag.saturating_sub_zero(previous.drag),
            key: self.key.saturating_sub_zero(previous.key),
            scroll: self.scroll.saturating_sub_zero(previous.scroll),
            left_modifier: self.left_modifier.saturating_delta(&previous.left_modifier),
            right_modifier: self
                .right_modifier
                .saturating_delta(&previous.right_modifier),
        }
    }

    /// Sum of every per-activity and per-modifier credit field.
    pub fn total(&self) -> Credit {
        self.click
            + self.drag
            + self.key
            + self.scroll
            + self.left_modifier.total()
            + self.right_modifier.total()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SplitCreditSnapshot {
    pub base: CreditSnapshot,
    pub boost: CreditSnapshot,
}

impl AddAssign<&CreditIncrement> for SplitCreditSnapshot {
    fn add_assign(&mut self, increment: &CreditIncrement) {
        self.base += &increment.base;
        self.boost += &increment.boost;
    }
}

impl Add<&CreditIncrement> for SplitCreditSnapshot {
    type Output = Self;

    fn add(mut self, increment: &CreditIncrement) -> Self {
        self += increment;
        self
    }
}

impl SplitCreditSnapshot {
    pub fn saturating_delta(&self, previous: &SplitCreditSnapshot) -> CreditIncrement {
        CreditIncrement {
            base: self.base.saturating_delta(&previous.base),
            boost: self.boost.saturating_delta(&previous.boost),
        }
    }

    pub fn total(&self) -> Credit {
        self.base.total() + self.boost.total()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_credit_total_sums_base_and_boost_snapshots() {
        let snapshot = SplitCreditSnapshot {
            base: CreditSnapshot {
                key: Credit::new(3.0),
                click: Credit::new(2.0),
                ..CreditSnapshot::default()
            },
            boost: CreditSnapshot {
                key: Credit::new(1.5),
                scroll: Credit::new(0.5),
                ..CreditSnapshot::default()
            },
        };

        assert_eq!(snapshot.total(), Credit::new(7.0));
    }
}
