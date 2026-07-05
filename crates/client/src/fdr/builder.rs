use super::record::UsageCreditBucket;
use crate::credit::CreditIncrement;
use jiff::{SignedDuration, Timestamp};
use shared::model::{Credit, UsageDelta};
use shared::protocol::server::UsageIncrement;
use std::time::Duration;

/// Nominal logical bucket length. A bucket's `target_end` is its
/// `bucket_start` plus this; the actual `bucket_end` is the end of the last
/// included increment, so buckets can run slightly short or long.
const BUCKET_TARGET: SignedDuration = SignedDuration::from_secs(5);

/// The currently open bucket. By construction it always holds at least one
/// increment, so an empty-but-open bucket is unrepresentable: the builder
/// stores `Option<OpenUsageCreditBucket>` and only ever puts a `Some` there
/// via [`OpenUsageCreditBucket::start`].
struct OpenUsageCreditBucket {
    bucket_start: Timestamp,
    target_end: Timestamp,
    last_increment_end: Timestamp,
    usage: UsageDelta,
    credit: CreditIncrement,
    increment_count: u32,
    active_increment_count: u32,
    max_increment_key_count: u64,
    max_increment_click_count: u64,
    max_increment_scroll_count: u64,
    max_increment_total_credit: Credit,
    sum_increment_total_credit_squared: Credit,
    observed_duration: Duration,
}

impl OpenUsageCreditBucket {
    /// Open a fresh bucket seeded with its first increment.
    fn start(increment: &UsageIncrement, credit: &CreditIncrement) -> Self {
        let mut bucket = Self {
            bucket_start: increment.start,
            target_end: increment.start + BUCKET_TARGET,
            last_increment_end: increment.start,
            usage: UsageDelta::default(),
            credit: CreditIncrement::default(),
            increment_count: 0,
            active_increment_count: 0,
            max_increment_key_count: 0,
            max_increment_click_count: 0,
            max_increment_scroll_count: 0,
            max_increment_total_credit: Credit::ZERO,
            sum_increment_total_credit_squared: Credit::ZERO,
            observed_duration: Duration::ZERO,
        };
        bucket.add(increment, credit);
        bucket
    }

    /// Fold one raw increment into the open bucket.
    fn add(&mut self, increment: &UsageIncrement, credit: &CreditIncrement) {
        let delta = &increment.delta;
        self.usage += delta;
        self.credit += credit;

        self.increment_count += 1;
        self.last_increment_end = increment.end;
        self.observed_duration += increment.end.duration_since(increment.start).unsigned_abs();

        self.max_increment_key_count = self.max_increment_key_count.max(delta.key_count.total());
        self.max_increment_click_count = self.max_increment_click_count.max(delta.click_count);
        self.max_increment_scroll_count = self.max_increment_scroll_count.max(delta.scroll_count);

        let total_credit = credit.total();
        if total_credit > self.max_increment_total_credit {
            self.max_increment_total_credit = total_credit;
        }
        self.sum_increment_total_credit_squared += total_credit * total_credit;

        if usage_is_active(delta) || total_credit != Credit::ZERO {
            self.active_increment_count += 1;
        }
    }

    /// Materialize the finished record, or `None` when the bucket carried no
    /// usage and no credit (a no-op bucket that should not be written).
    fn finalize(self) -> Option<UsageCreditBucket> {
        if self.active_increment_count == 0 {
            return None;
        }
        Some(UsageCreditBucket {
            bucket_start: self.bucket_start,
            bucket_end: self.last_increment_end,
            increment_count: self.increment_count,
            u_click_count: self.usage.click_count,
            u_drag: self.usage.drag_duration,
            u_key_count: self.usage.key_count.total(),
            u_scroll_count: self.usage.scroll_count,
            u_left_shift: self.usage.left_modifier_duration.shift,
            u_left_ctrl: self.usage.left_modifier_duration.ctrl,
            u_left_alt: self.usage.left_modifier_duration.alt,
            u_left_meta: self.usage.left_modifier_duration.meta,
            u_right_shift: self.usage.right_modifier_duration.shift,
            u_right_ctrl: self.usage.right_modifier_duration.ctrl,
            u_right_alt: self.usage.right_modifier_duration.alt,
            u_right_meta: self.usage.right_modifier_duration.meta,
            u_active: self.usage.active_duration,
            cb_click: self.credit.base.click,
            cb_drag: self.credit.base.drag,
            cb_key: self.credit.base.key,
            cb_scroll: self.credit.base.scroll,
            cb_left_shift: self.credit.base.left_modifier.shift,
            cb_left_ctrl: self.credit.base.left_modifier.ctrl,
            cb_left_alt: self.credit.base.left_modifier.alt,
            cb_left_meta: self.credit.base.left_modifier.meta,
            cb_right_shift: self.credit.base.right_modifier.shift,
            cb_right_ctrl: self.credit.base.right_modifier.ctrl,
            cb_right_alt: self.credit.base.right_modifier.alt,
            cb_right_meta: self.credit.base.right_modifier.meta,
            cx_click: self.credit.boost.click,
            cx_drag: self.credit.boost.drag,
            cx_key: self.credit.boost.key,
            cx_scroll: self.credit.boost.scroll,
            cx_left_shift: self.credit.boost.left_modifier.shift,
            cx_left_ctrl: self.credit.boost.left_modifier.ctrl,
            cx_left_alt: self.credit.boost.left_modifier.alt,
            cx_left_meta: self.credit.boost.left_modifier.meta,
            cx_right_shift: self.credit.boost.right_modifier.shift,
            cx_right_ctrl: self.credit.boost.right_modifier.ctrl,
            cx_right_alt: self.credit.boost.right_modifier.alt,
            cx_right_meta: self.credit.boost.right_modifier.meta,
            max_increment_key_count: self.max_increment_key_count,
            max_increment_click_count: self.max_increment_click_count,
            max_increment_scroll_count: self.max_increment_scroll_count,
            max_increment_total_credit: self.max_increment_total_credit,
            sum_increment_total_credit_squared: self.sum_increment_total_credit_squared,
            active_increment_count: self.active_increment_count,
            observed_duration: self.observed_duration,
        })
    }
}

/// Aggregates raw `(UsageIncrement, CreditIncrement)` pairs into approximate
/// 5-second [`UsageCreditBucket`] records.
#[derive(Default)]
pub struct UsageCreditBucketBuilder {
    current: Option<OpenUsageCreditBucket>,
}

impl UsageCreditBucketBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one raw increment. Returns a finalized bucket when this
    /// increment crossed the current bucket's target end (and the closed
    /// bucket was not a no-op); otherwise returns `None`. The crossing
    /// increment is never split: it starts the next bucket whole.
    pub fn push(
        &mut self,
        increment: &UsageIncrement,
        credit: &CreditIncrement,
    ) -> Option<UsageCreditBucket> {
        match self.current.as_mut() {
            None => {
                self.current = Some(OpenUsageCreditBucket::start(increment, credit));
                None
            }
            Some(open) if increment.end <= open.target_end => {
                open.add(increment, credit);
                None
            }
            Some(open) => {
                // The increment crosses the target end of a non-empty bucket:
                // close the current bucket and start a new one at this
                // increment without splitting it.
                std::mem::replace(open, OpenUsageCreditBucket::start(increment, credit)).finalize()
            }
        }
    }

    /// Finalize the open bucket, if any. Used when the raw input closes or
    /// the recorder shuts down. Returns `None` when there is no open bucket
    /// or it was a no-op.
    pub fn finish(&mut self) -> Option<UsageCreditBucket> {
        self.current
            .take()
            .and_then(OpenUsageCreditBucket::finalize)
    }
}

/// Whether a usage delta represents any tracked input.
fn usage_is_active(delta: &UsageDelta) -> bool {
    delta.click_count != 0
        || delta.key_count.total() != 0
        || delta.scroll_count != 0
        || !delta.drag_duration.is_zero()
        || !delta.active_duration.is_zero()
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::model::KeyCount;

    fn ts(millis: i64) -> Timestamp {
        Timestamp::UNIX_EPOCH + SignedDuration::from_millis(millis)
    }

    /// Increment spanning `[start_ms, end_ms)` with the given key count.
    fn keys(start_ms: i64, end_ms: i64, key_count: u64) -> UsageIncrement {
        let delta = UsageDelta {
            key_count: KeyCount {
                left: key_count,
                ..KeyCount::default()
            },
            active_duration: Duration::from_millis((end_ms - start_ms) as u64),
            ..UsageDelta::default()
        };
        UsageIncrement::new(delta, ts(start_ms), ts(end_ms))
    }

    /// A credit increment with `base.key` and `boost.key` set, so the total
    /// credit is exactly `base + boost`.
    fn credit(base: f64, boost: f64) -> CreditIncrement {
        let mut increment = CreditIncrement::default();
        increment.base.key = Credit::new(base);
        increment.boost.key = Credit::new(boost);
        increment
    }

    #[test]
    fn accumulates_multiple_increments_into_one_bucket() {
        let mut builder = UsageCreditBucketBuilder::new();

        assert!(builder.push(&keys(0, 1000, 3), &credit(1.0, 0.5)).is_none());
        assert!(
            builder
                .push(&keys(1000, 2000, 2), &credit(2.0, 0.0))
                .is_none()
        );
        assert!(
            builder
                .push(&keys(2000, 3000, 5), &credit(0.5, 0.25))
                .is_none()
        );

        let bucket = builder.finish().expect("non-empty bucket");
        assert_eq!(bucket.increment_count, 3);
        assert_eq!(bucket.active_increment_count, 3);
        assert_eq!(bucket.u_key_count, 10);
        assert_eq!(bucket.bucket_start, ts(0));
        assert_eq!(bucket.bucket_end, ts(3000));
        // base.key totals: 1.0 + 2.0 + 0.5; boost.key totals: 0.5 + 0.0 + 0.25
        assert_eq!(bucket.cb_key.as_f64(), 3.5);
        assert_eq!(bucket.cx_key.as_f64(), 0.75);
        assert_eq!(bucket.observed_duration, Duration::from_millis(3000));
    }

    #[test]
    fn finalizes_when_next_increment_crosses_target_end() {
        let mut builder = UsageCreditBucketBuilder::new();

        // First bucket targets [0, 5000).
        assert!(builder.push(&keys(0, 2000, 1), &credit(1.0, 0.0)).is_none());
        assert!(
            builder
                .push(&keys(2000, 4000, 1), &credit(1.0, 0.0))
                .is_none()
        );

        // This increment ends at 6000 > 5000, so the first bucket finalizes.
        let first = builder
            .push(&keys(4000, 6000, 1), &credit(1.0, 0.0))
            .expect("first bucket finalized");
        assert_eq!(first.bucket_start, ts(0));
        assert_eq!(first.bucket_end, ts(4000));
        assert_eq!(first.increment_count, 2);

        // The crossing increment is not split: it begins the second bucket.
        let second = builder.finish().expect("second bucket");
        assert_eq!(second.bucket_start, ts(4000));
        assert_eq!(second.bucket_end, ts(6000));
        assert_eq!(second.increment_count, 1);
    }

    #[test]
    fn does_not_split_a_crossing_increment() {
        let mut builder = UsageCreditBucketBuilder::new();

        assert!(builder.push(&keys(0, 1000, 1), &credit(1.0, 0.0)).is_none());

        // A single long increment straddling the boundary lands entirely in
        // the new bucket rather than being divided.
        let first = builder
            .push(&keys(1000, 9000, 4), &credit(2.0, 0.0))
            .expect("first bucket finalized");
        assert_eq!(first.increment_count, 1);
        assert_eq!(first.u_key_count, 1);

        let second = builder.finish().expect("second bucket");
        assert_eq!(second.bucket_start, ts(1000));
        assert_eq!(second.bucket_end, ts(9000));
        assert_eq!(second.u_key_count, 4);
    }

    #[test]
    fn computes_max_and_squared_credit_stats() {
        let mut builder = UsageCreditBucketBuilder::new();

        builder.push(&keys(0, 1000, 2), &credit(1.0, 0.0)); // total 1.0
        builder.push(&keys(1000, 2000, 7), &credit(2.0, 1.0)); // total 3.0
        builder.push(&keys(2000, 3000, 4), &credit(0.0, 0.5)); // total 0.5

        let bucket = builder.finish().expect("non-empty bucket");
        assert_eq!(bucket.max_increment_key_count, 7);
        assert_eq!(bucket.max_increment_total_credit, Credit::new(3.0));
        // 1.0^2 + 3.0^2 + 0.5^2 = 1 + 9 + 0.25
        assert_eq!(
            bucket.sum_increment_total_credit_squared,
            Credit::new(10.25)
        );
    }

    #[test]
    fn finishes_non_empty_open_bucket_on_shutdown() {
        let mut builder = UsageCreditBucketBuilder::new();
        assert!(builder.push(&keys(0, 1000, 1), &credit(1.0, 0.0)).is_none());

        let bucket = builder.finish().expect("open bucket finalized on shutdown");
        assert_eq!(bucket.increment_count, 1);
        // A second finish has nothing left to emit.
        assert!(builder.finish().is_none());
    }

    #[test]
    fn skips_no_op_buckets() {
        let mut builder = UsageCreditBucketBuilder::new();

        // Increment with no usage and no credit: a no-op that must not be
        // emitted even though it opened a bucket.
        let idle = UsageIncrement::new(UsageDelta::default(), ts(0), ts(1000));
        assert!(builder.push(&idle, &CreditIncrement::default()).is_none());
        assert!(builder.finish().is_none());
    }
}
