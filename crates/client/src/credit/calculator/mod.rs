use self::config::{
    CreditCalculatorConfig, CreditCostConfig, CreditRateBoostConfig, GlobalCreditBoostConfig,
    ModifierCostConfig, RateBoostConfig,
};
use crate::credit::{CreditDelta, CreditIncrement, KeyCreditDelta, ModifierCreditDelta};
use shared::model::{Credit, ModifierUsageDelta};
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
        Self {
            local_rates: LocalRateTrackers::new(&config.rate_boost),
            global_rate: config
                .global_boost
                .enabled
                .then(|| GlobalBoostTracker::new(config.global_boost)),
            config,
        }
    }

    pub fn calculate(&mut self, increment: &UsageIncrement) -> CreditIncrement {
        let base = base_credit(increment, &self.config.costs);
        let delta = &increment.delta;

        let dt = delta.active_duration.as_secs_f64();
        if dt <= 0.0 {
            return CreditIncrement {
                base,
                boost: CreditDelta::default(),
            };
        }

        let key_boost_fraction = self
            .local_rates
            .key
            .boost_fraction(delta.key_event_count() as f64 / dt, dt);
        let click_boost_fraction = self
            .local_rates
            .click
            .boost_fraction(delta.click_count as f64 / dt, dt);
        let scroll_boost_fraction = self
            .local_rates
            .scroll
            .boost_fraction(delta.scroll_count as f64 / dt, dt);
        let drag_boost_fraction = self
            .local_rates
            .drag
            .boost_fraction(delta.drag_duration.as_secs_f64() / dt, dt);
        let left_modifier_boost_fraction = self.local_rates.left_modifier.boost_fraction(
            modifier_duration_secs(&delta.left_modifier_duration) / dt,
            dt,
        );
        let right_modifier_boost_fraction = self.local_rates.right_modifier.boost_fraction(
            modifier_duration_secs(&delta.right_modifier_duration) / dt,
            dt,
        );
        let global_boost_fraction = self
            .global_rate
            .as_mut()
            .map(|global_rate| global_rate.boost_fraction(base.total().as_f64() / dt, dt))
            .unwrap_or(0.0);

        let boost = CreditDelta {
            key: base.key.scaled(key_boost_fraction + global_boost_fraction),
            click: base.click * (click_boost_fraction + global_boost_fraction),
            scroll: base.scroll * (scroll_boost_fraction + global_boost_fraction),
            drag: base.drag * (drag_boost_fraction + global_boost_fraction),
            left_modifier: base
                .left_modifier
                .scaled(left_modifier_boost_fraction + global_boost_fraction),
            right_modifier: base
                .right_modifier
                .scaled(right_modifier_boost_fraction + global_boost_fraction),
        };
        CreditIncrement { base, boost }
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
    left_modifier: RateBoostTracker,
    right_modifier: RateBoostTracker,
}

impl LocalRateTrackers {
    fn new(config: &CreditRateBoostConfig) -> Self {
        Self {
            key: RateBoostTracker::new(config.key),
            click: RateBoostTracker::new(config.click),
            scroll: RateBoostTracker::new(config.scroll),
            drag: RateBoostTracker::new(config.drag),
            left_modifier: RateBoostTracker::new(config.left_modifier),
            right_modifier: RateBoostTracker::new(config.right_modifier),
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

fn base_credit(increment: &UsageIncrement, costs: &CreditCostConfig) -> CreditDelta {
    let delta = &increment.delta;
    CreditDelta {
        key: key_credit(delta, &costs.key),
        click: Credit::new(delta.click_count as f64 * costs.click.per_click),
        scroll: Credit::new(delta.scroll_count as f64 * costs.scroll.per_scroll),
        drag: Credit::new(delta.drag_duration.as_secs_f64() * costs.drag.per_sec),
        left_modifier: modifier_credit(&delta.left_modifier_duration, &costs.left_modifier),
        right_modifier: modifier_credit(&delta.right_modifier_duration, &costs.right_modifier),
    }
}

fn key_credit(delta: &shared::model::UsageDelta, costs: &config::KeyCostConfig) -> KeyCreditDelta {
    KeyCreditDelta {
        left: Credit::new(delta.key_count.left as f64 * costs.left),
        right: Credit::new(delta.key_count.right as f64 * costs.right),
        other: Credit::new(delta.key_count.other as f64 * costs.other),
        left_combo: Credit::new(delta.left_modifier_duration.combo as f64 * costs.left_combo),
        right_combo: Credit::new(delta.right_modifier_duration.combo as f64 * costs.right_combo),
        cross_combo: Credit::new(delta.cross_combo as f64 * costs.cross_combo),
        other_combo: Credit::new(delta.other_combo as f64 * costs.other_combo),
    }
}

fn modifier_credit(delta: &ModifierUsageDelta, costs: &ModifierCostConfig) -> ModifierCreditDelta {
    ModifierCreditDelta {
        shift: Credit::new(delta.shift.as_secs_f64() * costs.shift_per_sec),
        ctrl: Credit::new(delta.ctrl.as_secs_f64() * costs.ctrl_per_sec),
        alt: Credit::new(delta.alt.as_secs_f64() * costs.alt_per_sec),
        meta: Credit::new(delta.meta.as_secs_f64() * costs.meta_per_sec),
        multi: Credit::new(delta.multi.as_secs_f64() * costs.multi_per_sec),
    }
}

fn modifier_duration_secs(delta: &ModifierUsageDelta) -> f64 {
    delta.shift.as_secs_f64()
        + delta.ctrl.as_secs_f64()
        + delta.alt.as_secs_f64()
        + delta.meta.as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::config::*;
    use super::*;
    use jiff::Timestamp;
    use shared::model::{KeyCount, UsageDelta};
    use std::time::Duration;

    fn key_count(total: u64) -> KeyCount {
        KeyCount {
            left: total,
            ..KeyCount::default()
        }
    }

    fn unboosted_config() -> CreditCalculatorConfig {
        let mut config = CreditCalculatorConfig::default();
        config.rate_boost.key.enabled = false;
        config.rate_boost.click.enabled = false;
        config.rate_boost.scroll.enabled = false;
        config.rate_boost.drag.enabled = false;
        config.rate_boost.left_modifier.enabled = false;
        config.rate_boost.right_modifier.enabled = false;
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
            key_count: KeyCount {
                left: 3,
                right: 4,
                other: 5,
            },
            click_count: 2,
            scroll_count: 8,
            drag_duration: Duration::from_secs(2),
            left_modifier_duration: ModifierUsageDelta {
                shift: Duration::from_secs(1),
                alt: Duration::from_secs(2),
                multi: Duration::from_secs(3),
                ..ModifierUsageDelta::default()
            },
            right_modifier_duration: ModifierUsageDelta {
                ctrl: Duration::from_millis(500),
                ..ModifierUsageDelta::default()
            },
            ..UsageDelta::default()
        };

        let credit = calculator.calculate(&increment(delta, 0, 2));

        assert_close(credit.base.key.total().as_f64(), 12.0);
        assert_close(credit.base.click.as_f64(), 4.0);
        assert_close(credit.base.scroll.as_f64(), 2.0);
        assert_close(credit.base.drag.as_f64(), 6.0);
        assert_close(credit.base.left_modifier.shift.as_f64(), 5.0);
        assert_close(credit.base.left_modifier.alt.as_f64(), 6.0);
        assert_close(credit.base.left_modifier.multi.as_f64(), 3.0);
        assert_close(credit.base.right_modifier.ctrl.as_f64(), 2.5);
        assert_close(credit.boost.total().as_f64(), 0.0);
    }

    #[test]
    fn base_key_credit_uses_per_hand_and_combo_costs() {
        let mut config = unboosted_config();
        config.costs.key = KeyCostConfig {
            left: 1.0,
            right: 2.0,
            other: 3.0,
            left_combo: 4.0,
            right_combo: 5.0,
            cross_combo: 6.0,
            other_combo: 7.0,
        };
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                key_count: KeyCount {
                    left: 2,
                    right: 3,
                    other: 4,
                },
                left_modifier_duration: ModifierUsageDelta {
                    combo: 5,
                    ..ModifierUsageDelta::default()
                },
                right_modifier_duration: ModifierUsageDelta {
                    combo: 6,
                    ..ModifierUsageDelta::default()
                },
                cross_combo: 7,
                other_combo: 8,
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.key.left.as_f64(), 2.0);
        assert_close(credit.base.key.right.as_f64(), 6.0);
        assert_close(credit.base.key.other.as_f64(), 12.0);
        assert_close(credit.base.key.left_combo.as_f64(), 20.0);
        assert_close(credit.base.key.right_combo.as_f64(), 30.0);
        assert_close(credit.base.key.cross_combo.as_f64(), 42.0);
        assert_close(credit.base.key.other_combo.as_f64(), 56.0);
        assert_close(credit.base.key.total().as_f64(), 168.0);
        assert_close(credit.boost.total().as_f64(), 0.0);
    }

    #[test]
    fn modifier_credit_charges_multi_addon_separately() {
        let mut config = unboosted_config();
        config.costs.left_modifier.multi_per_sec = 0.5;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left_modifier_duration: ModifierUsageDelta {
                    shift: Duration::from_secs(2),
                    multi: Duration::from_secs(4),
                    ..ModifierUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.left_modifier.shift.as_f64(), 10.0);
        assert_close(credit.base.left_modifier.multi.as_f64(), 2.0);
        assert_close(credit.base.left_modifier.total().as_f64(), 12.0);
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
                key_count: key_count(2),
                click_count: 1,
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.key.total().as_f64(), 2.0);
        assert!(
            credit.boost.key.total().as_f64() > 5.99,
            "key boost should be reported separately: {credit:?}"
        );
        assert_close(credit.base.click.as_f64(), 2.0);
        assert_close(credit.boost.click.as_f64(), 0.0);
    }

    #[test]
    fn key_local_boost_rate_includes_combo_events() {
        let mut config = unboosted_config();
        config.rate_boost.key.enabled = true;
        config.rate_boost.key.baseline_per_sec = 0.5;
        config.rate_boost.key.factor = 1.0;
        config.rate_boost.key.cap = 10.0;
        config.rate_boost.key.smoothing_secs = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left_modifier_duration: ModifierUsageDelta {
                    combo: 2,
                    ..ModifierUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.key.total().as_f64(), 2.5);
        assert!(
            credit.boost.key.total().as_f64() > 7.49,
            "combo key events should drive key boost: {credit:?}"
        );
    }

    #[test]
    fn modifier_local_boost_rate_excludes_multi_overlap() {
        let mut config = unboosted_config();
        config.rate_boost.left_modifier.enabled = true;
        config.rate_boost.left_modifier.baseline_per_sec = 1.5;
        config.rate_boost.left_modifier.factor = 1.0;
        config.rate_boost.left_modifier.cap = 10.0;
        config.rate_boost.left_modifier.smoothing_secs = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left_modifier_duration: ModifierUsageDelta {
                    multi: Duration::from_secs(2),
                    ..ModifierUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.left_modifier.multi.as_f64(), 2.0);
        assert_close(credit.boost.left_modifier.multi.as_f64(), 0.0);
    }

    #[test]
    fn modifier_local_boost_scales_multi_credit_when_duration_drives_rate() {
        let mut config = unboosted_config();
        config.rate_boost.left_modifier.enabled = true;
        config.rate_boost.left_modifier.baseline_per_sec = 0.5;
        config.rate_boost.left_modifier.factor = 1.0;
        config.rate_boost.left_modifier.cap = 10.0;
        config.rate_boost.left_modifier.smoothing_secs = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left_modifier_duration: ModifierUsageDelta {
                    shift: Duration::from_secs(1),
                    multi: Duration::from_secs(1),
                    ..ModifierUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.left_modifier.multi.as_f64(), 1.0);
        assert_close(credit.boost.left_modifier.multi.as_f64(), 1.0);
    }

    #[test]
    fn modifier_local_boost_rate_excludes_combo_count() {
        let mut config = unboosted_config();
        config.rate_boost.left_modifier.enabled = true;
        config.rate_boost.left_modifier.baseline_per_sec = 0.5;
        config.rate_boost.left_modifier.factor = 1.0;
        config.rate_boost.left_modifier.cap = 10.0;
        config.rate_boost.left_modifier.smoothing_secs = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left_modifier_duration: ModifierUsageDelta {
                    combo: 4,
                    multi: Duration::from_secs(1),
                    ..ModifierUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.key.total().as_f64(), 5.0);
        assert_close(credit.base.left_modifier.multi.as_f64(), 1.0);
        assert_close(credit.boost.left_modifier.multi.as_f64(), 0.0);
    }

    #[test]
    fn zero_duration_uses_base_credit_without_boost() {
        let mut config = unboosted_config();
        config.rate_boost.key.enabled = true;
        config.rate_boost.key.baseline_per_sec = 0.01;
        let mut calculator = CreditCalculator::new(config);

        let credit = calculator.calculate(&increment(
            UsageDelta {
                left_modifier_duration: ModifierUsageDelta {
                    combo: 10,
                    ..ModifierUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            1,
            1,
        ));

        assert_close(credit.base.key.total().as_f64(), 12.5);
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

        let credit = calculator.calculate(&increment(
            UsageDelta {
                key_count: key_count(2),
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.key.total().as_f64(), 2.0);
        assert!(
            credit.boost.key.total().as_f64() > 1.99,
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

        let credit = calculator.calculate(&increment(
            UsageDelta {
                key_count: key_count(2),
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.key.total().as_f64(), 2.0);
        assert_close(credit.boost.key.total().as_f64(), 8.0);
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

        let credit = calculator.calculate(&increment(
            UsageDelta {
                key_count: key_count(2),
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.key.total().as_f64(), 2.0);
        assert!(
            credit.boost.key.total().as_f64() > 5.99,
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
                left_modifier_duration: ModifierUsageDelta {
                    combo: 1,
                    multi: Duration::from_secs(1),
                    ..ModifierUsageDelta::default()
                },
                ..UsageDelta::default()
            },
            0,
            1,
        ));

        assert_close(credit.base.key.total().as_f64(), 1.25);
        assert_close(credit.base.left_modifier.multi.as_f64(), 1.0);
        assert!(
            credit.boost.total().as_f64() > 0.24,
            "global boost should see full base credit: {credit:?}"
        );
    }
}
