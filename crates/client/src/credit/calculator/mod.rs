use self::config::{
    CreditCalculatorConfig, CreditRateBoostConfig, GlobalCreditBoostConfig, HandCostConfig,
    ModifierCostConfig, RateBoostConfig, ResolvedCreditCosts,
};
use crate::credit::{CreditDelta, CreditIncrement, HandCreditDelta};
use shared::model::{Credit, HandUsageDelta, ModifierUsageDelta};
use shared::protocol::server::UsageIncrement;

pub mod config;

#[derive(Debug)]
pub struct CreditCalculator {
    config: CreditCalculatorConfig,
    local_rates: LocalRateTrackers,
    global_rate: Option<GlobalBoostTracker>,
}

impl CreditCalculator {
    pub fn new(config: CreditCalculatorConfig) -> Self {
        let global_rate = config
            .global_boost
            .enabled
            .then(|| GlobalBoostTracker::new(config.global_boost.clone()));

        Self {
            local_rates: LocalRateTrackers::new(&config.rate_boost),
            global_rate,
            config,
        }
    }

    pub fn calculate(&mut self, increment: &UsageIncrement) -> CreditIncrement {
        let base = base_credit(increment, &self.config.costs);
        let delta = &increment.delta;

        let dt = delta.active_duration.as_secs_f64();
        if dt <= 0.0 {
            return CreditIncrement {
                base: base.compact(),
                boost: CreditDelta::default(),
            };
        }

        let key_boost_fraction = self
            .local_rates
            .key
            .boost_fraction(delta.key_event_count() as f64 / dt, dt);
        let click_boost_fraction = self.local_rates.click.boost_fraction(
            (delta.left.click_count + delta.right.click_count) as f64 / dt,
            dt,
        );
        let scroll_boost_fraction = self.local_rates.scroll.boost_fraction(
            (delta.left.scroll_count + delta.right.scroll_count) as f64 / dt,
            dt,
        );
        let drag_boost_fraction = self.local_rates.drag.boost_fraction(
            (delta.left.drag_duration.as_secs_f64() + delta.right.drag_duration.as_secs_f64()) / dt,
            dt,
        );
        let modifier_boost_fraction = self.local_rates.modifier.boost_fraction(
            (modifier_duration_secs(&delta.left.modifier)
                + modifier_duration_secs(&delta.right.modifier))
                / dt,
            dt,
        );
        let global_boost_fraction = self
            .global_rate
            .as_mut()
            .map(|global_rate| global_rate.boost_fraction(base.total().as_f64() / dt, dt))
            .unwrap_or(0.0);

        let boost = base.boosted(
            key_boost_fraction,
            click_boost_fraction,
            scroll_boost_fraction,
            drag_boost_fraction,
            modifier_boost_fraction,
            global_boost_fraction,
        );
        CreditIncrement {
            base: base.compact(),
            boost,
        }
    }
}

impl Default for CreditCalculator {
    fn default() -> Self {
        Self::new(CreditCalculatorConfig::default())
    }
}

#[derive(Debug)]
struct LocalRateTrackers {
    key: RateBoostTracker,
    click: RateBoostTracker,
    scroll: RateBoostTracker,
    drag: RateBoostTracker,
    modifier: RateBoostTracker,
}

impl LocalRateTrackers {
    fn new(config: &CreditRateBoostConfig) -> Self {
        Self {
            key: RateBoostTracker::new(config.key.clone()),
            click: RateBoostTracker::new(config.click.clone()),
            scroll: RateBoostTracker::new(config.scroll.clone()),
            drag: RateBoostTracker::new(config.drag.clone()),
            modifier: RateBoostTracker::new(config.modifier.clone()),
        }
    }
}

#[derive(Debug)]
struct RateBoostTracker {
    config: RateBoostConfig,
    ema: RateEma,
}

impl RateBoostTracker {
    fn new(config: RateBoostConfig) -> Self {
        Self {
            config,
            ema: RateEma::default(),
        }
    }

    fn boost_fraction(&mut self, instant_rate: f64, dt: f64) -> f64 {
        if !self.config.enabled {
            return 0.0;
        }
        let smoothed_rate = self
            .ema
            .update(instant_rate, dt, self.config.smoothing_secs);
        boost_fraction(
            smoothed_rate,
            self.config.baseline_per_sec,
            self.config.factor,
            self.config.cap,
        )
    }
}

#[derive(Debug)]
struct GlobalBoostTracker {
    config: GlobalCreditBoostConfig,
    ema: RateEma,
}

impl GlobalBoostTracker {
    fn new(config: GlobalCreditBoostConfig) -> Self {
        Self {
            config,
            ema: RateEma::default(),
        }
    }

    fn boost_fraction(&mut self, instant_rate: f64, dt: f64) -> f64 {
        let smoothed_rate = self
            .ema
            .update(instant_rate, dt, self.config.smoothing_secs);
        boost_fraction(
            smoothed_rate,
            self.config.baseline_credit_per_sec,
            self.config.factor,
            self.config.cap,
        )
    }
}

#[derive(Debug, Default)]
struct RateEma {
    value: f64,
}

impl RateEma {
    fn update(&mut self, instant_rate: f64, dt: f64, smoothing_secs: f64) -> f64 {
        let alpha = 1.0 - (-dt / smoothing_secs).exp();
        self.value += alpha * (instant_rate - self.value);
        self.value
    }
}

fn boost_fraction(rate: f64, baseline: f64, factor: f64, cap: f64) -> f64 {
    let over_baseline = (rate / baseline - 1.0).max(0.0);
    (over_baseline * factor).min(cap - 1.0)
}

#[derive(Debug, Clone, Copy, Default)]
struct ExpandedCreditDelta {
    left: ExpandedHandCreditDelta,
    right: ExpandedHandCreditDelta,
    unclassified_key: Credit,
    unclassified_combo: Credit,
}

impl ExpandedCreditDelta {
    fn total(&self) -> Credit {
        self.left.total() + self.right.total() + self.unclassified_key + self.unclassified_combo
    }

    fn compact(&self) -> CreditDelta {
        CreditDelta {
            left: self.left.compact(),
            right: self.right.compact(),
            unclassified_key: self.unclassified_key + self.unclassified_combo,
        }
    }

    fn boosted(
        &self,
        key_boost_fraction: f64,
        click_boost_fraction: f64,
        scroll_boost_fraction: f64,
        drag_boost_fraction: f64,
        modifier_boost_fraction: f64,
        global_boost_fraction: f64,
    ) -> CreditDelta {
        ExpandedCreditDelta {
            left: self.left.boosted(
                key_boost_fraction,
                click_boost_fraction,
                scroll_boost_fraction,
                drag_boost_fraction,
                modifier_boost_fraction,
                global_boost_fraction,
            ),
            right: self.right.boosted(
                key_boost_fraction,
                click_boost_fraction,
                scroll_boost_fraction,
                drag_boost_fraction,
                modifier_boost_fraction,
                global_boost_fraction,
            ),
            unclassified_key: boosted_credit(
                self.unclassified_key,
                key_boost_fraction,
                global_boost_fraction,
            ),
            unclassified_combo: boosted_credit(
                self.unclassified_combo,
                key_boost_fraction,
                global_boost_fraction,
            ),
        }
        .compact()
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ExpandedHandCreditDelta {
    click: Credit,
    drag: Credit,
    key: Credit,
    scroll: Credit,
    modifier_duration: Credit,
    same_hand_combo: Credit,
}

impl ExpandedHandCreditDelta {
    fn total(&self) -> Credit {
        self.click
            + self.drag
            + self.key
            + self.scroll
            + self.modifier_duration
            + self.same_hand_combo
    }

    fn compact(&self) -> HandCreditDelta {
        HandCreditDelta {
            click: self.click,
            drag: self.drag,
            key: self.key,
            scroll: self.scroll,
            modifier: self.modifier_duration + self.same_hand_combo,
        }
    }

    fn boosted(
        &self,
        key_boost_fraction: f64,
        click_boost_fraction: f64,
        scroll_boost_fraction: f64,
        drag_boost_fraction: f64,
        modifier_boost_fraction: f64,
        global_boost_fraction: f64,
    ) -> Self {
        Self {
            click: boosted_credit(self.click, click_boost_fraction, global_boost_fraction),
            drag: boosted_credit(self.drag, drag_boost_fraction, global_boost_fraction),
            key: boosted_credit(self.key, key_boost_fraction, global_boost_fraction),
            scroll: boosted_credit(self.scroll, scroll_boost_fraction, global_boost_fraction),
            modifier_duration: boosted_credit(
                self.modifier_duration,
                modifier_boost_fraction,
                global_boost_fraction,
            ),
            same_hand_combo: boosted_credit(
                self.same_hand_combo,
                key_boost_fraction,
                global_boost_fraction,
            ),
        }
    }
}

fn boosted_credit(credit: Credit, local_boost_fraction: f64, global_boost_fraction: f64) -> Credit {
    credit * (local_boost_fraction + global_boost_fraction)
}

fn base_credit(increment: &UsageIncrement, costs: &ResolvedCreditCosts) -> ExpandedCreditDelta {
    let delta = &increment.delta;
    ExpandedCreditDelta {
        left: hand_credit(&delta.left, &costs.left),
        right: hand_credit(&delta.right, &costs.right),
        unclassified_key: Credit::new(delta.unclassified_key_count as f64 * costs.unclassified.key),
        unclassified_combo: Credit::new(
            delta.unclassified_key_combo as f64 * costs.unclassified.combo,
        ),
    }
}

fn hand_credit(delta: &HandUsageDelta, costs: &HandCostConfig) -> ExpandedHandCreditDelta {
    ExpandedHandCreditDelta {
        click: Credit::new(delta.click_count as f64 * costs.click),
        drag: Credit::new(delta.drag_duration.as_secs_f64() * costs.drag_per_sec),
        key: Credit::new(delta.key_count as f64 * costs.key),
        scroll: Credit::new(delta.scroll_count as f64 * costs.scroll),
        modifier_duration: modifier_credit(&delta.modifier, &costs.modifier),
        same_hand_combo: Credit::new(delta.modifier.same_hand_combo as f64 * costs.same_hand_combo),
    }
}

fn modifier_credit(delta: &ModifierUsageDelta, costs: &ModifierCostConfig) -> Credit {
    Credit::new(delta.shift.as_secs_f64() * costs.shift_per_sec)
        + Credit::new(delta.ctrl.as_secs_f64() * costs.ctrl_per_sec)
        + Credit::new(delta.alt.as_secs_f64() * costs.alt_per_sec)
        + Credit::new(delta.meta.as_secs_f64() * costs.meta_per_sec)
        + Credit::new(delta.multi.as_secs_f64() * costs.multi_per_sec)
}

fn modifier_duration_secs(delta: &ModifierUsageDelta) -> f64 {
    delta.shift.as_secs_f64()
        + delta.ctrl.as_secs_f64()
        + delta.alt.as_secs_f64()
        + delta.meta.as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Timestamp;
    use shared::model::{HandUsageDelta, ModifierUsageDelta, UsageDelta};
    use std::time::Duration;

    fn left_key_delta(total: u64) -> UsageDelta {
        UsageDelta {
            left: HandUsageDelta {
                key_count: total,
                ..HandUsageDelta::default()
            },
            ..UsageDelta::default()
        }
    }

    fn unboosted_config() -> CreditCalculatorConfig {
        let mut config = CreditCalculatorConfig::default();
        config.rate_boost.key.enabled = false;
        config.rate_boost.click.enabled = false;
        config.rate_boost.scroll.enabled = false;
        config.rate_boost.drag.enabled = false;
        config.rate_boost.modifier.enabled = false;
        config.global_boost.enabled = false;
        config
    }

    fn increment(mut delta: UsageDelta, start_second: i64, end_second: i64) -> UsageIncrement {
        let start = Timestamp::from_second(start_second).expect("valid start timestamp");
        let end = Timestamp::from_second(end_second).expect("valid end timestamp");
        if delta.active_duration.is_zero() {
            let span = end.duration_since(start);
            delta.active_duration = span.try_into().unwrap_or_default();
        }
        UsageIncrement::new(delta, start, end)
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 0.000_001,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn base_credit_is_linear_in_usage_amount() {
        let mut calculator = CreditCalculator::new(unboosted_config());
        let delta = UsageDelta {
            left: HandUsageDelta {
                click_count: 2,
                drag_duration: Duration::from_secs(2),
                key_count: 3,
                scroll_count: 8,
                modifier: ModifierUsageDelta {
                    shift: Duration::from_secs(1),
                    alt: Duration::from_secs(2),
                    multi: Duration::from_secs(3),
                    ..ModifierUsageDelta::default()
                },
            },
            right: HandUsageDelta {
                click_count: 3,
                drag_duration: Duration::from_secs(1),
                key_count: 4,
                scroll_count: 4,
                modifier: ModifierUsageDelta {
                    ctrl: Duration::from_millis(500),
                    ..ModifierUsageDelta::default()
                },
            },
            unclassified_key_count: 5,
            unclassified_key_combo: 6,
            ..UsageDelta::default()
        };

        let credit = calculator.calculate(&increment(delta, 0, 2));

        assert_close(credit.base.left.key.as_f64(), 3.0);
        assert_close(credit.base.right.key.as_f64(), 4.0);
        assert_close(credit.base.unclassified_key.as_f64(), 11.6);
        assert_close(credit.base.left.click.as_f64(), 4.0);
        assert_close(credit.base.right.click.as_f64(), 6.0);
        assert_close(credit.base.left.scroll.as_f64(), 2.0);
        assert_close(credit.base.right.scroll.as_f64(), 1.0);
        assert_close(credit.base.left.drag.as_f64(), 6.0);
        assert_close(credit.base.right.drag.as_f64(), 3.0);
        assert_close(credit.base.left.modifier.as_f64(), 14.0);
        assert_close(credit.base.right.modifier.as_f64(), 2.5);
        assert_close(credit.boost.total().as_f64(), 0.0);
    }

    #[test]
    fn base_key_credit_uses_per_hand_combo_and_unclassified_costs() {
        let mut config = unboosted_config();
        config.costs.left.key = 1.0;
        config.costs.right.key = 2.0;
        config.costs.left.same_hand_combo = 4.0;
        config.costs.right.same_hand_combo = 5.0;
        config.costs.unclassified.key = 3.0;
        config.costs.unclassified.combo = 7.0;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left: HandUsageDelta {
                    key_count: 2,
                    modifier: ModifierUsageDelta {
                        same_hand_combo: 5,
                        ..ModifierUsageDelta::default()
                    },
                    ..HandUsageDelta::default()
                },
                right: HandUsageDelta {
                    key_count: 3,
                    modifier: ModifierUsageDelta {
                        same_hand_combo: 6,
                        ..ModifierUsageDelta::default()
                    },
                    ..HandUsageDelta::default()
                },
                unclassified_key_count: 4,
                unclassified_key_combo: 8,
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.left.key.as_f64(), 2.0);
        assert_close(credit.base.right.key.as_f64(), 6.0);
        assert_close(credit.base.left.modifier.as_f64(), 20.0);
        assert_close(credit.base.right.modifier.as_f64(), 30.0);
        assert_close(credit.base.unclassified_key.as_f64(), 68.0);
        assert_close(credit.base.total().as_f64(), 126.0);
        assert_close(credit.boost.total().as_f64(), 0.0);
    }

    #[test]
    fn modifier_credit_charges_multi_addon_separately() {
        let mut config = unboosted_config();
        config.costs.left.modifier.multi_per_sec = 0.5;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left: HandUsageDelta {
                    modifier: ModifierUsageDelta {
                        shift: Duration::from_secs(2),
                        multi: Duration::from_secs(4),
                        ..ModifierUsageDelta::default()
                    },
                    ..HandUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.left.modifier.as_f64(), 12.0);
    }

    #[test]
    fn local_rate_boost_applies_to_matching_channel() {
        let mut config = unboosted_config();
        config.rate_boost.key.enabled = true;
        config.rate_boost.key.baseline_per_sec = 0.5;
        config.rate_boost.key.factor = 1.0;
        config.rate_boost.key.cap = 10.0;
        config.rate_boost.key.smoothing_secs = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left: HandUsageDelta {
                    click_count: 1,
                    key_count: 2,
                    ..HandUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.left.key.as_f64(), 2.0);
        assert!(
            credit.boost.left.key.as_f64() > 5.99,
            "key boost should be reported separately: {credit:?}"
        );
        assert_close(credit.base.left.click.as_f64(), 2.0);
        assert_close(credit.boost.left.click.as_f64(), 0.0);
    }

    #[test]
    fn key_local_boost_rate_includes_combo_and_unclassified_events() {
        let mut config = unboosted_config();
        config.rate_boost.key.enabled = true;
        config.rate_boost.key.baseline_per_sec = 0.5;
        config.rate_boost.key.factor = 1.0;
        config.rate_boost.key.cap = 10.0;
        config.rate_boost.key.smoothing_secs = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left: HandUsageDelta {
                    modifier: ModifierUsageDelta {
                        same_hand_combo: 1,
                        ..ModifierUsageDelta::default()
                    },
                    ..HandUsageDelta::default()
                },
                unclassified_key_combo: 1,
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.left.modifier.as_f64(), 1.25);
        assert_close(credit.base.unclassified_key.as_f64(), 1.1);
        assert!(
            credit.boost.total().as_f64() > 7.04,
            "combo and unclassified key events should drive key boost: {credit:?}"
        );
    }

    #[test]
    fn modifier_local_boost_rate_excludes_multi_overlap() {
        let mut config = unboosted_config();
        config.rate_boost.modifier.enabled = true;
        config.rate_boost.modifier.baseline_per_sec = 1.5;
        config.rate_boost.modifier.factor = 1.0;
        config.rate_boost.modifier.cap = 10.0;
        config.rate_boost.modifier.smoothing_secs = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left: HandUsageDelta {
                    modifier: ModifierUsageDelta {
                        multi: Duration::from_secs(2),
                        ..ModifierUsageDelta::default()
                    },
                    ..HandUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.left.modifier.as_f64(), 2.0);
        assert_close(credit.boost.left.modifier.as_f64(), 0.0);
    }

    #[test]
    fn modifier_local_boost_scales_multi_credit_when_duration_drives_rate() {
        let mut config = unboosted_config();
        config.rate_boost.modifier.enabled = true;
        config.rate_boost.modifier.baseline_per_sec = 0.5;
        config.rate_boost.modifier.factor = 1.0;
        config.rate_boost.modifier.cap = 10.0;
        config.rate_boost.modifier.smoothing_secs = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left: HandUsageDelta {
                    modifier: ModifierUsageDelta {
                        shift: Duration::from_secs(1),
                        multi: Duration::from_secs(1),
                        ..ModifierUsageDelta::default()
                    },
                    ..HandUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.left.modifier.as_f64(), 6.0);
        assert_close(credit.boost.left.modifier.as_f64(), 6.0);
    }

    #[test]
    fn modifier_local_boost_rate_excludes_same_hand_combo_count() {
        let mut config = unboosted_config();
        config.rate_boost.modifier.enabled = true;
        config.rate_boost.modifier.baseline_per_sec = 0.5;
        config.rate_boost.modifier.factor = 1.0;
        config.rate_boost.modifier.cap = 10.0;
        config.rate_boost.modifier.smoothing_secs = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left: HandUsageDelta {
                    modifier: ModifierUsageDelta {
                        same_hand_combo: 4,
                        multi: Duration::from_secs(1),
                        ..ModifierUsageDelta::default()
                    },
                    ..HandUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.left.modifier.as_f64(), 6.0);
        assert_close(credit.boost.left.modifier.as_f64(), 0.0);
    }

    #[test]
    fn zero_duration_uses_base_credit_without_boost() {
        let mut config = unboosted_config();
        config.rate_boost.key.enabled = true;
        config.rate_boost.key.baseline_per_sec = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left: HandUsageDelta {
                    modifier: ModifierUsageDelta {
                        same_hand_combo: 10,
                        ..ModifierUsageDelta::default()
                    },
                    ..HandUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            1,
            1,
        ));

        assert_close(credit.base.left.modifier.as_f64(), 12.5);
        assert_close(credit.boost.total().as_f64(), 0.0);
    }

    #[test]
    fn global_boost_adds_to_boost_credit() {
        let mut config = unboosted_config();
        config.global_boost.enabled = true;
        config.global_boost.baseline_credit_per_sec = 1.0;
        config.global_boost.factor = 1.0;
        config.global_boost.cap = 10.0;
        config.global_boost.smoothing_secs = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(left_key_delta(2), 0, 1));

        assert_close(credit.base.left.key.as_f64(), 2.0);
        assert!(
            credit.boost.left.key.as_f64() > 1.99,
            "global boost should be reported separately: {credit:?}"
        );
    }

    #[test]
    fn local_and_global_boost_fractions_are_additive() {
        let mut config = unboosted_config();
        config.rate_boost.key.enabled = true;
        config.rate_boost.key.baseline_per_sec = 0.5;
        config.rate_boost.key.factor = 1.0;
        config.rate_boost.key.cap = 10.0;
        config.rate_boost.key.smoothing_secs = 0.01;
        config.global_boost.enabled = true;
        config.global_boost.baseline_credit_per_sec = 1.0;
        config.global_boost.factor = 1.0;
        config.global_boost.cap = 10.0;
        config.global_boost.smoothing_secs = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(left_key_delta(2), 0, 1));

        assert_close(credit.base.left.key.as_f64(), 2.0);
        assert_close(credit.boost.left.key.as_f64(), 8.0);
    }

    #[test]
    fn global_boost_uses_base_rate_before_local_boosting() {
        let mut config = unboosted_config();
        config.rate_boost.key.enabled = true;
        config.rate_boost.key.baseline_per_sec = 0.5;
        config.rate_boost.key.factor = 1.0;
        config.rate_boost.key.cap = 10.0;
        config.rate_boost.key.smoothing_secs = 0.01;
        config.global_boost.enabled = true;
        config.global_boost.baseline_credit_per_sec = 3.0;
        config.global_boost.factor = 1.0;
        config.global_boost.cap = 10.0;
        config.global_boost.smoothing_secs = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(left_key_delta(2), 0, 1));

        assert_close(credit.base.left.key.as_f64(), 2.0);
        assert!(
            credit.boost.left.key.as_f64() > 5.99,
            "local boost should still apply: {credit:?}"
        );
        assert!(
            credit.total() < Credit::new(8.1),
            "global boost should not be triggered by local-effective rate: {credit:?}"
        );
    }

    #[test]
    fn global_boost_rate_includes_combo_and_multi_base_costs() {
        let mut config = unboosted_config();
        config.global_boost.enabled = true;
        config.global_boost.baseline_credit_per_sec = 2.0;
        config.global_boost.factor = 1.0;
        config.global_boost.cap = 10.0;
        config.global_boost.smoothing_secs = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left: HandUsageDelta {
                    modifier: ModifierUsageDelta {
                        same_hand_combo: 1,
                        multi: Duration::from_secs(1),
                        ..ModifierUsageDelta::default()
                    },
                    ..HandUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.left.modifier.as_f64(), 2.25);
        assert!(
            credit.boost.total().as_f64() > 0.24,
            "global boost should see full base credit: {credit:?}"
        );
    }
}
