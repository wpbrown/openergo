pub mod calculator;
pub mod limit;
pub mod utilization;

pub use calculator::CreditCalculator;
pub use calculator::config::CreditCalculatorConfig;
use serde::{Deserialize, Serialize};
pub use shared::model::Credit;
use std::ops::{Add, AddAssign};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct KeyCreditDelta {
    pub left: Credit,
    pub right: Credit,
    pub other: Credit,
    pub left_combo: Credit,
    pub right_combo: Credit,
    pub cross_combo: Credit,
    pub other_combo: Credit,
}

impl AddAssign<&KeyCreditDelta> for KeyCreditDelta {
    fn add_assign(&mut self, delta: &KeyCreditDelta) {
        self.left += delta.left;
        self.right += delta.right;
        self.other += delta.other;
        self.left_combo += delta.left_combo;
        self.right_combo += delta.right_combo;
        self.cross_combo += delta.cross_combo;
        self.other_combo += delta.other_combo;
    }
}

impl Add<&KeyCreditDelta> for KeyCreditDelta {
    type Output = Self;

    fn add(mut self, delta: &KeyCreditDelta) -> Self {
        self += delta;
        self
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ModifierCreditDelta {
    pub shift: Credit,
    pub ctrl: Credit,
    pub alt: Credit,
    pub meta: Credit,
    pub multi: Credit,
}

impl AddAssign<&ModifierCreditDelta> for ModifierCreditDelta {
    fn add_assign(&mut self, delta: &ModifierCreditDelta) {
        self.shift += delta.shift;
        self.ctrl += delta.ctrl;
        self.alt += delta.alt;
        self.meta += delta.meta;
        self.multi += delta.multi;
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
    pub key: KeyCreditDelta,
    pub scroll: Credit,
    pub left_modifier: ModifierCreditDelta,
    pub right_modifier: ModifierCreditDelta,
}

impl AddAssign<&CreditDelta> for CreditDelta {
    fn add_assign(&mut self, delta: &CreditDelta) {
        self.click += delta.click;
        self.drag += delta.drag;
        self.key += &delta.key;
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

impl KeyCreditDelta {
    pub fn total(&self) -> Credit {
        self.left
            + self.right
            + self.other
            + self.left_combo
            + self.right_combo
            + self.cross_combo
            + self.other_combo
    }

    pub fn scaled(&self, multiplier: f64) -> Self {
        Self {
            left: self.left * multiplier,
            right: self.right * multiplier,
            other: self.other * multiplier,
            left_combo: self.left_combo * multiplier,
            right_combo: self.right_combo * multiplier,
            cross_combo: self.cross_combo * multiplier,
            other_combo: self.other_combo * multiplier,
        }
    }
}

impl ModifierCreditDelta {
    pub fn total(&self) -> Credit {
        self.shift + self.ctrl + self.alt + self.meta + self.multi
    }

    pub fn scaled(&self, multiplier: f64) -> Self {
        Self {
            shift: self.shift * multiplier,
            ctrl: self.ctrl * multiplier,
            alt: self.alt * multiplier,
            meta: self.meta * multiplier,
            multi: self.multi * multiplier,
        }
    }
}

impl CreditDelta {
    pub fn total(&self) -> Credit {
        self.click
            + self.drag
            + self.key.total()
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
pub struct KeyCreditSnapshot {
    pub left: Credit,
    pub right: Credit,
    pub other: Credit,
    pub left_combo: Credit,
    pub right_combo: Credit,
    pub cross_combo: Credit,
    pub other_combo: Credit,
}

impl AddAssign<&KeyCreditDelta> for KeyCreditSnapshot {
    fn add_assign(&mut self, delta: &KeyCreditDelta) {
        self.left += delta.left;
        self.right += delta.right;
        self.other += delta.other;
        self.left_combo += delta.left_combo;
        self.right_combo += delta.right_combo;
        self.cross_combo += delta.cross_combo;
        self.other_combo += delta.other_combo;
    }
}

impl Add<&KeyCreditDelta> for KeyCreditSnapshot {
    type Output = Self;

    fn add(mut self, delta: &KeyCreditDelta) -> Self {
        self += delta;
        self
    }
}

impl KeyCreditSnapshot {
    pub fn saturating_delta(&self, previous: &KeyCreditSnapshot) -> KeyCreditDelta {
        KeyCreditDelta {
            left: self.left.saturating_sub_zero(previous.left),
            right: self.right.saturating_sub_zero(previous.right),
            other: self.other.saturating_sub_zero(previous.other),
            left_combo: self.left_combo.saturating_sub_zero(previous.left_combo),
            right_combo: self.right_combo.saturating_sub_zero(previous.right_combo),
            cross_combo: self.cross_combo.saturating_sub_zero(previous.cross_combo),
            other_combo: self.other_combo.saturating_sub_zero(previous.other_combo),
        }
    }

    pub fn total(&self) -> Credit {
        self.left
            + self.right
            + self.other
            + self.left_combo
            + self.right_combo
            + self.cross_combo
            + self.other_combo
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ModifierCreditSnapshot {
    pub shift: Credit,
    pub ctrl: Credit,
    pub alt: Credit,
    pub meta: Credit,
    pub multi: Credit,
}

impl AddAssign<&ModifierCreditDelta> for ModifierCreditSnapshot {
    fn add_assign(&mut self, delta: &ModifierCreditDelta) {
        self.shift += delta.shift;
        self.ctrl += delta.ctrl;
        self.alt += delta.alt;
        self.meta += delta.meta;
        self.multi += delta.multi;
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
            multi: self.multi.saturating_sub_zero(previous.multi),
        }
    }

    /// Sum of all modifier credit fields.
    pub fn total(&self) -> Credit {
        self.shift + self.ctrl + self.alt + self.meta + self.multi
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreditSnapshot {
    pub click: Credit,
    pub drag: Credit,
    pub key: KeyCreditSnapshot,
    pub scroll: Credit,
    pub left_modifier: ModifierCreditSnapshot,
    pub right_modifier: ModifierCreditSnapshot,
}

impl AddAssign<&CreditDelta> for CreditSnapshot {
    fn add_assign(&mut self, delta: &CreditDelta) {
        self.click += delta.click;
        self.drag += delta.drag;
        self.key += &delta.key;
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
            key: self.key.saturating_delta(&previous.key),
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
            + self.key.total()
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
    fn key_credit_participates_in_arithmetic_and_deltas() {
        let delta = KeyCreditDelta {
            left: Credit::new(1.0),
            left_combo: Credit::new(2.0),
            ..KeyCreditDelta::default()
        };
        let scaled = delta.scaled(1.5);

        assert_eq!(delta.total(), Credit::new(3.0));
        assert_eq!(scaled.left, Credit::new(1.5));
        assert_eq!(scaled.left_combo, Credit::new(3.0));

        let mut snapshot = KeyCreditSnapshot::default();
        snapshot += &delta;
        let previous = KeyCreditSnapshot {
            left_combo: Credit::new(0.5),
            ..KeyCreditSnapshot::default()
        };

        assert_eq!(snapshot.total(), Credit::new(3.0));
        assert_eq!(
            snapshot.saturating_delta(&previous).left_combo,
            Credit::new(1.5)
        );
    }

    #[test]
    fn modifier_multi_credit_participates_in_arithmetic_and_deltas() {
        let delta = ModifierCreditDelta {
            shift: Credit::new(1.0),
            multi: Credit::new(2.0),
            ..ModifierCreditDelta::default()
        };
        let scaled = delta.scaled(1.5);

        assert_eq!(delta.total(), Credit::new(3.0));
        assert_eq!(scaled.shift, Credit::new(1.5));
        assert_eq!(scaled.multi, Credit::new(3.0));

        let mut snapshot = ModifierCreditSnapshot::default();
        snapshot += &delta;
        let previous = ModifierCreditSnapshot {
            multi: Credit::new(0.5),
            ..ModifierCreditSnapshot::default()
        };

        assert_eq!(snapshot.total(), Credit::new(3.0));
        assert_eq!(snapshot.saturating_delta(&previous).multi, Credit::new(1.5));
    }

    #[test]
    fn split_credit_total_sums_base_and_boost_snapshots() {
        let snapshot = SplitCreditSnapshot {
            base: CreditSnapshot {
                key: KeyCreditSnapshot {
                    left: Credit::new(3.0),
                    ..KeyCreditSnapshot::default()
                },
                click: Credit::new(2.0),
                ..CreditSnapshot::default()
            },
            boost: CreditSnapshot {
                key: KeyCreditSnapshot {
                    left_combo: Credit::new(1.5),
                    ..KeyCreditSnapshot::default()
                },
                scroll: Credit::new(0.5),
                ..CreditSnapshot::default()
            },
        };

        assert_eq!(snapshot.total(), Credit::new(7.0));
    }
}
