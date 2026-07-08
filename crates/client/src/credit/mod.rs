pub mod calculator;
pub mod limit;
pub mod utilization;

pub use calculator::CreditCalculator;
pub use calculator::config::CreditCalculatorConfig;
use serde::{Deserialize, Serialize};
pub use shared::model::Credit;
use std::ops::{Add, AddAssign};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CreditDelta {
    pub left: HandCreditDelta,
    pub right: HandCreditDelta,
    pub unclassified_key: Credit,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct HandCreditDelta {
    pub click: Credit,
    pub drag: Credit,
    pub key: Credit,
    pub scroll: Credit,
    pub modifier: Credit,
}

impl AddAssign<&HandCreditDelta> for HandCreditDelta {
    fn add_assign(&mut self, delta: &HandCreditDelta) {
        self.click += delta.click;
        self.drag += delta.drag;
        self.key += delta.key;
        self.scroll += delta.scroll;
        self.modifier += delta.modifier;
    }
}

impl Add<&HandCreditDelta> for HandCreditDelta {
    type Output = Self;

    fn add(mut self, delta: &HandCreditDelta) -> Self {
        self += delta;
        self
    }
}

impl AddAssign<&CreditDelta> for CreditDelta {
    fn add_assign(&mut self, delta: &CreditDelta) {
        self.left += &delta.left;
        self.right += &delta.right;
        self.unclassified_key += delta.unclassified_key;
    }
}

impl Add<&CreditDelta> for CreditDelta {
    type Output = Self;

    fn add(mut self, delta: &CreditDelta) -> Self {
        self += delta;
        self
    }
}

impl HandCreditDelta {
    pub fn total(&self) -> Credit {
        self.click + self.drag + self.key + self.scroll + self.modifier
    }
}

impl CreditDelta {
    pub fn total(&self) -> Credit {
        self.left.total() + self.right.total() + self.unclassified_key
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
pub struct CreditSnapshot {
    pub left: HandCreditSnapshot,
    pub right: HandCreditSnapshot,
    pub unclassified_key: Credit,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct HandCreditSnapshot {
    pub click: Credit,
    pub drag: Credit,
    pub key: Credit,
    pub scroll: Credit,
    pub modifier: Credit,
}

impl AddAssign<&HandCreditDelta> for HandCreditSnapshot {
    fn add_assign(&mut self, delta: &HandCreditDelta) {
        self.click += delta.click;
        self.drag += delta.drag;
        self.key += delta.key;
        self.scroll += delta.scroll;
        self.modifier += delta.modifier;
    }
}

impl Add<&HandCreditDelta> for HandCreditSnapshot {
    type Output = Self;

    fn add(mut self, delta: &HandCreditDelta) -> Self {
        self += delta;
        self
    }
}

impl HandCreditSnapshot {
    pub fn saturating_delta(&self, previous: &HandCreditSnapshot) -> HandCreditDelta {
        HandCreditDelta {
            click: self.click.saturating_sub_zero(previous.click),
            drag: self.drag.saturating_sub_zero(previous.drag),
            key: self.key.saturating_sub_zero(previous.key),
            scroll: self.scroll.saturating_sub_zero(previous.scroll),
            modifier: self.modifier.saturating_sub_zero(previous.modifier),
        }
    }

    pub fn total(&self) -> Credit {
        self.click + self.drag + self.key + self.scroll + self.modifier
    }
}

impl AddAssign<&CreditDelta> for CreditSnapshot {
    fn add_assign(&mut self, delta: &CreditDelta) {
        self.left += &delta.left;
        self.right += &delta.right;
        self.unclassified_key += delta.unclassified_key;
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
            left: self.left.saturating_delta(&previous.left),
            right: self.right.saturating_delta(&previous.right),
            unclassified_key: self
                .unclassified_key
                .saturating_sub_zero(previous.unclassified_key),
        }
    }

    pub fn total(&self) -> Credit {
        self.left.total() + self.right.total() + self.unclassified_key
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
    fn hand_credit_participates_in_arithmetic_and_deltas() {
        let delta = HandCreditDelta {
            key: Credit::new(1.0),
            modifier: Credit::new(2.0),
            ..HandCreditDelta::default()
        };

        assert_eq!(delta.total(), Credit::new(3.0));

        let mut snapshot = HandCreditSnapshot::default();
        snapshot += &delta;
        let previous = HandCreditSnapshot {
            modifier: Credit::new(0.5),
            ..HandCreditSnapshot::default()
        };

        assert_eq!(snapshot.total(), Credit::new(3.0));
        assert_eq!(
            snapshot.saturating_delta(&previous).modifier,
            Credit::new(1.5)
        );
    }

    #[test]
    fn compact_credit_participates_in_arithmetic_and_deltas() {
        let delta = CreditDelta {
            left: HandCreditDelta {
                key: Credit::new(1.0),
                ..HandCreditDelta::default()
            },
            unclassified_key: Credit::new(2.0),
            ..CreditDelta::default()
        };

        assert_eq!(delta.total(), Credit::new(3.0));

        let mut snapshot = CreditSnapshot::default();
        snapshot += &delta;
        let previous = CreditSnapshot {
            unclassified_key: Credit::new(0.5),
            ..CreditSnapshot::default()
        };

        assert_eq!(snapshot.total(), Credit::new(3.0));
        assert_eq!(
            snapshot.saturating_delta(&previous).unclassified_key,
            Credit::new(1.5)
        );
    }

    #[test]
    fn split_credit_total_sums_base_and_boost_snapshots() {
        let snapshot = SplitCreditSnapshot {
            base: CreditSnapshot {
                left: HandCreditSnapshot {
                    key: Credit::new(3.0),
                    ..HandCreditSnapshot::default()
                },
                unclassified_key: Credit::new(2.0),
                ..CreditSnapshot::default()
            },
            boost: CreditSnapshot {
                left: HandCreditSnapshot {
                    modifier: Credit::new(1.5),
                    ..HandCreditSnapshot::default()
                },
                right: HandCreditSnapshot {
                    scroll: Credit::new(0.5),
                    ..HandCreditSnapshot::default()
                },
                ..CreditSnapshot::default()
            },
        };

        assert_eq!(snapshot.total(), Credit::new(7.0));
    }
}
