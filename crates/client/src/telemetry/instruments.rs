use crate::credit::limit::CreditLimitConsumer;
use crate::credit::{
    CreditDelta, CreditIncrement, KeyCreditDelta, ModifierCreditDelta, SplitCreditSnapshot,
};
use crate::pain::PainConsumer;
use opentelemetry::metrics::{Counter, Gauge};
use opentelemetry::{KeyValue, global};
use shared::model::{ModifierUsageDelta, UsageDelta};
use std::time::Duration;

pub struct Instruments {
    clicks: Counter<u64>,
    drag_duration: Counter<f64>,
    key_presses: Counter<u64>,
    scroll_ticks: Counter<u64>,
    modifier_duration: Counter<f64>,
    credit_all: Counter<f64>,
    credit_rest_usage: Gauge<f64>,
    credit_break_usage: Gauge<f64>,
    credit_day_usage: Gauge<f64>,
    credit_rest_limit: Gauge<f64>,
    credit_break_limit: Gauge<f64>,
    credit_day_limit: Gauge<f64>,
    pain_reported: Gauge<f64>,
    activity_duration: Counter<f64>,
}

impl Instruments {
    pub fn new() -> Self {
        let meter = global::meter("openergo");
        Self {
            clicks: meter
                .u64_counter("usage.clicks")
                .with_unit("{click}")
                .build(),
            drag_duration: meter
                .f64_counter("usage.drag.duration")
                .with_unit("s")
                .build(),
            key_presses: meter
                .u64_counter("usage.key_presses")
                .with_unit("{press}")
                .build(),
            scroll_ticks: meter
                .u64_counter("usage.scroll_ticks")
                .with_unit("{tick}")
                .build(),
            modifier_duration: meter
                .f64_counter("usage.modifier.duration")
                .with_unit("s")
                .build(),
            credit_all: meter
                .f64_counter("credit.all")
                .with_unit("{credit}")
                .build(),
            credit_rest_usage: meter
                .f64_gauge("credit.rest.usage")
                .with_unit("{credit}")
                .build(),
            credit_break_usage: meter
                .f64_gauge("credit.break.usage")
                .with_unit("{credit}")
                .build(),
            credit_day_usage: meter
                .f64_gauge("credit.day.usage")
                .with_unit("{credit}")
                .build(),
            credit_rest_limit: meter
                .f64_gauge("credit.rest.limit")
                .with_unit("{credit}")
                .build(),
            credit_break_limit: meter
                .f64_gauge("credit.break.limit")
                .with_unit("{credit}")
                .build(),
            credit_day_limit: meter
                .f64_gauge("credit.day.limit")
                .with_unit("{credit}")
                .build(),
            activity_duration: meter
                .f64_counter("activity.duration")
                .with_unit("s")
                .build(),
            pain_reported: meter.f64_gauge("pain.reported").with_unit("1").build(),
        }
    }

    pub fn record_usage(&self, delta: &UsageDelta) {
        self.clicks.add(delta.click_count, &[]);
        self.drag_duration
            .add(delta.drag_duration.as_secs_f64(), &[]);
        self.key_presses.add(delta.key_event_count(), &[]);
        self.scroll_ticks.add(delta.scroll_count, &[]);

        self.record_modifier_duration("left", &delta.left_modifier_duration);
        self.record_modifier_duration("right", &delta.right_modifier_duration);
    }

    fn record_modifier_duration(&self, side: &'static str, delta: &ModifierUsageDelta) {
        let attrs = |key: &'static str| [KeyValue::new("side", side), KeyValue::new("key", key)];
        self.modifier_duration
            .add(delta.shift.as_secs_f64(), &attrs("shift"));
        self.modifier_duration
            .add(delta.ctrl.as_secs_f64(), &attrs("ctrl"));
        self.modifier_duration
            .add(delta.alt.as_secs_f64(), &attrs("alt"));
        self.modifier_duration
            .add(delta.meta.as_secs_f64(), &attrs("meta"));
        self.modifier_duration
            .add(delta.multi.as_secs_f64(), &attrs("multi"));
    }

    pub fn record_credit(&self, increment: &CreditIncrement) {
        self.record_credit_delta("base", &increment.base);
        self.record_credit_delta("boost", &increment.boost);
    }

    fn record_credit_delta(&self, credit_type: &'static str, delta: &CreditDelta) {
        self.credit_all
            .add(delta.click.as_f64(), &credit_attrs(credit_type, "click"));
        self.credit_all
            .add(delta.drag.as_f64(), &credit_attrs(credit_type, "drag"));
        self.credit_all
            .add(delta.scroll.as_f64(), &credit_attrs(credit_type, "scroll"));

        self.record_key_credit(credit_type, &delta.key);
        self.record_modifier_credit(credit_type, "left", &delta.left_modifier);
        self.record_modifier_credit(credit_type, "right", &delta.right_modifier);
    }

    fn record_key_credit(&self, credit_type: &'static str, delta: &KeyCreditDelta) {
        let attrs = |key: &'static str| {
            [
                KeyValue::new("type", credit_type),
                KeyValue::new("source", "key"),
                KeyValue::new("key", key),
            ]
        };
        self.credit_all.add(delta.left.as_f64(), &attrs("left"));
        self.credit_all.add(delta.right.as_f64(), &attrs("right"));
        self.credit_all.add(delta.other.as_f64(), &attrs("other"));
        self.credit_all
            .add(delta.left_combo.as_f64(), &attrs("left_combo"));
        self.credit_all
            .add(delta.right_combo.as_f64(), &attrs("right_combo"));
        self.credit_all
            .add(delta.cross_combo.as_f64(), &attrs("cross_combo"));
        self.credit_all
            .add(delta.other_combo.as_f64(), &attrs("other_combo"));
    }

    fn record_modifier_credit(
        &self,
        credit_type: &'static str,
        side: &'static str,
        delta: &ModifierCreditDelta,
    ) {
        let attrs = |key: &'static str| {
            [
                KeyValue::new("type", credit_type),
                KeyValue::new("source", "modifier"),
                KeyValue::new("side", side),
                KeyValue::new("key", key),
            ]
        };
        self.credit_all.add(delta.shift.as_f64(), &attrs("shift"));
        self.credit_all.add(delta.ctrl.as_f64(), &attrs("ctrl"));
        self.credit_all.add(delta.alt.as_f64(), &attrs("alt"));
        self.credit_all.add(delta.meta.as_f64(), &attrs("meta"));
        self.credit_all.add(delta.multi.as_f64(), &attrs("multi"));
    }

    pub fn record_credit_gauges(
        &self,
        rest: &SplitCreditSnapshot,
        breaks: &SplitCreditSnapshot,
        day: &SplitCreditSnapshot,
        limits: &CreditLimitConsumer,
    ) {
        self.record_split_credit_gauge(&self.credit_rest_usage, rest);
        self.record_split_credit_gauge(&self.credit_break_usage, breaks);
        self.record_split_credit_gauge(&self.credit_day_usage, day);
        limits.view(|state| {
            self.credit_rest_limit.record(state.rest.as_f64(), &[]);
            self.credit_break_limit.record(state.breaks.as_f64(), &[]);
            self.credit_day_limit.record(state.day.as_f64(), &[]);
        });
    }

    fn record_split_credit_gauge(&self, gauge: &Gauge<f64>, credit: &SplitCreditSnapshot) {
        gauge.record(
            credit.base.total().as_f64(),
            &[KeyValue::new("type", "base")],
        );
        gauge.record(
            credit.boost.total().as_f64(),
            &[KeyValue::new("type", "boost")],
        );
    }

    pub fn record_pain(&self, pain: &PainConsumer) {
        pain.view(|state, catalog| {
            for (label, entry) in &state.entries {
                self.pain_reported.record(
                    entry.ratio(),
                    &[
                        KeyValue::new("label", catalog.resolve(*label)),
                        KeyValue::new("bias", catalog.bias_of(*label).as_str()),
                    ],
                );
            }
        });
    }

    pub fn record_activity(&self, delta: Duration) {
        self.activity_duration.add(delta.as_secs_f64(), &[]);
    }
}

fn credit_attrs(credit_type: &'static str, source: &'static str) -> [KeyValue; 2] {
    [
        KeyValue::new("type", credit_type),
        KeyValue::new("source", source),
    ]
}
