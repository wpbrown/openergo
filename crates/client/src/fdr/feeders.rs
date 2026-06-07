use super::builder::UsageCreditBucketBuilder;
use super::record::{
    ActivityBucket, CreditEventKind, CreditEventRecord, CreditLimitChange, CreditWindowState,
    FdrRecord, PainChange,
};
use crate::activity::ActivityStateConsumer;
use crate::credit::limit::{CreditLimitConsumer, CreditLimitState};
use crate::credit::utilization::{CreditEvent, CreditEventConsumer, CreditUtilizationConsumer};
use crate::pain::{PainConsumer, PainLabel};
use crate::usage::{AllUsageConsumer, UsageRawConsumer};
use bachelor::channel::mpsc::MpscChannelProducer;
use bachelor::error::Closed;
use jiff::Timestamp;
use std::collections::BTreeMap;
use std::pin::pin;
use std::time::Duration;

/// Activity sampling cadence, matching the plan's 30-second activity bucket.
const ACTIVITY_SAMPLE_INTERVAL: Duration = Duration::from_secs(30);

/// Send one record, returning `false` if the channel is closed (the writer
/// task has gone away). Feeders treat a closed channel as a reason to exit;
/// the writer task itself surfaces any underlying error.
async fn emit(tx: &MpscChannelProducer<FdrRecord>, record: FdrRecord) -> bool {
    tx.send(record).await.is_ok()
}

/// Usage+credit bucket feeder. Owns the [`UsageCreditBucketBuilder`] and
/// drives it from the raw `(UsageIncrement, CreditIncrement)` broadcast,
/// sending each finalized non-empty bucket. On input close or shutdown it
/// finalizes the open bucket before returning.
pub async fn usage_bucket(mut usage_raw: UsageRawConsumer, tx: MpscChannelProducer<FdrRecord>) {
    let mut builder = UsageCreditBucketBuilder::new();

    loop {
        let recv = pin!(usage_raw.recv_ref(|(increment, credit)| builder.push(increment, credit)));
        match recv.await {
            Ok(Some(bucket)) => {
                if !emit(&tx, FdrRecord::UsageCredit(Box::new(bucket))).await {
                    return;
                }
            }
            Ok(None) => {}
            Err(Closed) => break,
        }
    }

    if let Some(bucket) = builder.finish() {
        let _ = emit(&tx, FdrRecord::UsageCredit(Box::new(bucket))).await;
    }
}

/// Activity feeder. Follows the telemetry-style cumulative delta: it remembers
/// the previous cumulative activity total and emits at most one
/// [`ActivityBucket`] per 30-second window while activity changes arrive.
/// On input closure it emits a final partial sample so the last sub-interval
/// of activity is not lost.
pub async fn activity_sampler(
    mut activity: ActivityStateConsumer,
    tx: MpscChannelProducer<FdrRecord>,
) {
    let mut prev_total = activity.view(|state| state.total());
    let mut window_start = Timestamp::now();

    while activity.changed().await.is_ok() {
        let now = Timestamp::now();
        if now.duration_since(window_start).unsigned_abs() < ACTIVITY_SAMPLE_INTERVAL {
            continue;
        }

        let current = activity.view(|state| state.total());
        let delta = current.saturating_sub(prev_total);
        prev_total = current;
        if !delta.is_zero() {
            let record = ActivityBucket {
                bucket_start: window_start,
                bucket_end: now,
                activity_delta: delta,
            };
            if !emit(&tx, FdrRecord::Activity(record)).await {
                return;
            }
        }
        window_start = now;
    }

    let now = Timestamp::now();
    let delta = activity
        .view(|state| state.total())
        .saturating_sub(prev_total);
    if !delta.is_zero() {
        let record = ActivityBucket {
            bucket_start: window_start,
            bucket_end: now,
            activity_delta: delta,
        };
        let _ = emit(&tx, FdrRecord::Activity(record)).await;
    }
}

/// Credit-window feeder. Emits an initial [`CreditWindowState`] during setup,
/// then emits a new record whenever the accumulated rest/break/day credit
/// totals change. Deduplicates on the totals only, so the per-send
/// `recorded_at` timestamp does not defeat the change detection.
pub async fn credit_window(mut usage: AllUsageConsumer, tx: MpscChannelProducer<FdrRecord>) {
    let mut last = window_totals(&usage);
    if !emit(&tx, FdrRecord::CreditWindowState(window_state(last))).await {
        return;
    }

    while usage.changed().await.is_ok() {
        let current = window_totals(&usage);
        if current != last {
            last = current;
            if !emit(&tx, FdrRecord::CreditWindowState(window_state(current))).await {
                return;
            }
        }
    }
}

/// Pain feeder. Emits an initial [`PainChange`] per committed pain ratio
/// during setup, then emits one `PainChange` per label whose committed
/// `ratio` changed. The raw `live` value is intentionally ignored; only the
/// debounced committed ratio is recorded.
pub async fn pain(mut pain: PainConsumer, tx: MpscChannelProducer<FdrRecord>) {
    let mut last_by_label: BTreeMap<PainLabel, f64> = BTreeMap::new();

    for change in diff_pain(&pain, &mut last_by_label) {
        if !emit(&tx, FdrRecord::PainChange(Box::new(change))).await {
            return;
        }
    }

    while pain.changed().await.is_ok() {
        for change in diff_pain(&pain, &mut last_by_label) {
            if !emit(&tx, FdrRecord::PainChange(Box::new(change))).await {
                return;
            }
        }
    }
}

/// Credit-limit feeder. Emits an initial [`CreditLimitChange`] during setup,
/// then emits a new record whenever any limit changes.
pub async fn credit_limit(mut limits: CreditLimitConsumer, tx: MpscChannelProducer<FdrRecord>) {
    let mut last = limits.view(|state| *state);
    if !emit(&tx, FdrRecord::CreditLimitChange(limit_change(&last))).await {
        return;
    }

    while limits.changed().await.is_ok() {
        let current = limits.view(|state| *state);
        if current != last {
            last = current;
            if !emit(&tx, FdrRecord::CreditLimitChange(limit_change(&current))).await {
                return;
            }
        }
    }
}

/// Credit-event feeder. Forwards exactly one [`CreditEventRecord`] per
/// received [`CreditEvent`], attaching the current rest/break/day
/// utilization context viewed at event-receipt time.
pub async fn credit_event(
    mut events: CreditEventConsumer,
    utilization: CreditUtilizationConsumer,
    tx: MpscChannelProducer<FdrRecord>,
) {
    loop {
        let recv = pin!(events.recv());
        match recv.await {
            Ok(event) => {
                let record = event_record(event, &utilization);
                if !emit(&tx, FdrRecord::CreditEvent(record)).await {
                    return;
                }
            }
            Err(Closed) => break,
        }
    }
}

/// Current accumulated rest/break/day credit totals.
fn window_totals(usage: &AllUsageConsumer) -> (f64, f64, f64) {
    usage.view(|_all, rest, breaks, day| {
        (
            rest.credit().total().as_f64(),
            breaks.credit().total().as_f64(),
            day.credit().total().as_f64(),
        )
    })
}

/// Stamp a fresh `CreditWindowState` from the given totals.
fn window_state((rest, breaks, day): (f64, f64, f64)) -> CreditWindowState {
    CreditWindowState {
        recorded_at: Timestamp::now(),
        rest_credit_total: rest,
        break_credit_total: breaks,
        day_credit_total: day,
    }
}

/// Build the `PainChange` records for every label whose committed ratio
/// differs from `last_by_label`, updating the map in place. On the first
/// call `last_by_label` is empty, so every existing label is emitted.
fn diff_pain(pain: &PainConsumer, last_by_label: &mut BTreeMap<PainLabel, f64>) -> Vec<PainChange> {
    pain.view(|state, catalog| {
        let mut changes = Vec::new();
        for (label, entry) in &state.entries {
            let ratio = entry.ratio();
            if last_by_label.get(label) != Some(&ratio) {
                last_by_label.insert(*label, ratio);
                changes.push(PainChange {
                    recorded_at: Timestamp::now(),
                    label: catalog.resolve(*label).to_string(),
                    bias: catalog.bias_of(*label),
                    ratio,
                    last_updated: entry.last_updated(),
                });
            }
        }
        changes
    })
}

fn limit_change(state: &CreditLimitState) -> CreditLimitChange {
    CreditLimitChange {
        recorded_at: Timestamp::now(),
        rest: state.rest.0,
        breaks: state.breaks.0,
        day: state.day.0,
    }
}

fn event_record(event: CreditEvent, utilization: &CreditUtilizationConsumer) -> CreditEventRecord {
    let (kind, event_kind, level) = match event {
        CreditEvent::Reached { kind } => (kind, CreditEventKind::Reached, None),
        CreditEvent::Escalation { kind, level } => (kind, CreditEventKind::Escalation, Some(level)),
        CreditEvent::Reset { kind } => (kind, CreditEventKind::Reset, None),
    };

    let (rest_utilization, break_utilization, day_utilization) = utilization.view(|state| {
        let u = state.last_published();
        (u.rest, u.breaks, u.day)
    });

    CreditEventRecord {
        recorded_at: Timestamp::now(),
        kind,
        event: event_kind,
        level,
        rest_utilization,
        break_utilization,
        day_utilization,
    }
}
