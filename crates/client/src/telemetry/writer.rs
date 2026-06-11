use super::instruments::Instruments;
use crate::activity::ActivityStateConsumer;
use crate::credit::SplitCreditSnapshot;
use crate::credit::limit::CreditLimitConsumer;
use crate::pain::PainConsumer;
use crate::usage::AllUsageConsumer;
use crate::watch_mux::{WatchMux, define_watch_mux};
use futures::future::{Either, select};
use rootcause::prelude::*;
use shared::model::UsageSnapshot;
use std::pin::pin;
use std::time::Duration;
use tokio::time::{Instant, MissedTickBehavior};

const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);
const METRIC_EXPORT_INTERVAL_NAME: &str = "OTEL_METRIC_EXPORT_INTERVAL";

define_watch_mux! {
    struct TelemetryInputs;
    flags TelemetryInput;
    usage: AllUsageConsumer => USAGE,
    pain: PainConsumer => PAIN,
    limits: CreditLimitConsumer => LIMITS,
    activity: ActivityStateConsumer => ACTIVITY,
}

struct TelemetrySnapshot {
    usage: UsageSnapshot,
    credit: SplitCreditSnapshot,
    activity: Duration,
}

pub fn create(
    consumer: AllUsageConsumer,
    pain: PainConsumer,
    limits: CreditLimitConsumer,
    activity: ActivityStateConsumer,
) -> impl Future<Output = Result<(), Report>> {
    run(consumer, pain, limits, activity)
}

pub async fn wait_closed(
    usage: AllUsageConsumer,
    pain: PainConsumer,
    limits: CreditLimitConsumer,
    activity: ActivityStateConsumer,
) {
    let mut inputs = WatchMux::new(TelemetryInputs {
        usage,
        pain,
        limits,
        activity,
    });
    inputs.closed().await;
}

async fn run(
    usage: AllUsageConsumer,
    pain: PainConsumer,
    limits: CreditLimitConsumer,
    activity: ActivityStateConsumer,
) -> Result<(), Report> {
    let report_interval = std::env::var(METRIC_EXPORT_INTERVAL_NAME)
        .ok()
        .and_then(|v| v.parse().map(Duration::from_millis).ok())
        .unwrap_or(DEFAULT_INTERVAL);

    let instruments = Instruments::new();
    let mut inputs = WatchMux::new(TelemetryInputs {
        usage,
        pain,
        limits,
        activity,
    });
    let mut previous = {
        let inputs = inputs.get();
        let (usage, credit): (UsageSnapshot, SplitCreditSnapshot) =
            inputs.usage.view(|all, rest, breaks, day| {
                instruments.record_credit_gauges(
                    rest.credit(),
                    breaks.credit(),
                    day.credit(),
                    &inputs.limits,
                );
                (all.usage().clone(), all.credit().clone())
            });
        instruments.record_pain(&inputs.pain);
        TelemetrySnapshot {
            usage,
            credit,
            activity: inputs.activity.view(|state| state.total()),
        }
    };

    // Skip the immediate first tick: gauges have just been recorded above and
    // the counter delta would be zero.
    let mut interval = tokio::time::interval_at(Instant::now() + report_interval, report_interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        let closed = {
            let tick = pin!(interval.tick());
            let closed = pin!(inputs.closed());
            matches!(select(tick, closed).await, Either::Right(((), _)))
        };

        record_delta(&instruments, inputs.get(), &mut previous);
        if closed {
            return Ok(());
        }
    }
}

fn record_delta(
    instruments: &Instruments,
    inputs: &TelemetryInputs,
    previous: &mut TelemetrySnapshot,
) {
    let (current_usage, current_credit) = inputs.usage.view(|all, rest, breaks, day| {
        instruments.record_credit_gauges(
            rest.credit(),
            breaks.credit(),
            day.credit(),
            &inputs.limits,
        );
        (all.usage().clone(), all.credit().clone())
    });
    instruments.record_pain(&inputs.pain);

    let usage_delta = current_usage.saturating_delta(&previous.usage);
    let credit_delta = current_credit.saturating_delta(&previous.credit);
    instruments.record_usage(&usage_delta);
    instruments.record_credit(&credit_delta);
    previous.usage = current_usage;
    previous.credit = current_credit;

    let current_activity = inputs.activity.view(|state| state.total());
    let activity_delta = current_activity.saturating_sub(previous.activity);
    instruments.record_activity(activity_delta);
    previous.activity = current_activity;
}
