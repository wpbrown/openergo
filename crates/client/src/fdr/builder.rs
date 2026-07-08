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
    max_increment_left_key_count: u64,
    max_increment_right_key_count: u64,
    max_increment_unclassified_key_count: u64,
    max_increment_left_combo_count: u64,
    max_increment_right_combo_count: u64,
    max_increment_unclassified_combo_count: u64,
    max_increment_left_click_count: u64,
    max_increment_right_click_count: u64,
    max_increment_left_scroll_count: u64,
    max_increment_right_scroll_count: u64,
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
            max_increment_left_key_count: 0,
            max_increment_right_key_count: 0,
            max_increment_unclassified_key_count: 0,
            max_increment_left_combo_count: 0,
            max_increment_right_combo_count: 0,
            max_increment_unclassified_combo_count: 0,
            max_increment_left_click_count: 0,
            max_increment_right_click_count: 0,
            max_increment_left_scroll_count: 0,
            max_increment_right_scroll_count: 0,
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

        self.max_increment_left_key_count =
            self.max_increment_left_key_count.max(delta.left.key_count);
        self.max_increment_right_key_count = self
            .max_increment_right_key_count
            .max(delta.right.key_count);
        self.max_increment_unclassified_key_count = self
            .max_increment_unclassified_key_count
            .max(delta.unclassified_key_count);
        self.max_increment_left_combo_count = self
            .max_increment_left_combo_count
            .max(delta.left.modifier.same_hand_combo);
        self.max_increment_right_combo_count = self
            .max_increment_right_combo_count
            .max(delta.right.modifier.same_hand_combo);
        self.max_increment_unclassified_combo_count = self
            .max_increment_unclassified_combo_count
            .max(delta.unclassified_key_combo);
        self.max_increment_left_click_count = self
            .max_increment_left_click_count
            .max(delta.left.click_count);
        self.max_increment_right_click_count = self
            .max_increment_right_click_count
            .max(delta.right.click_count);
        self.max_increment_left_scroll_count = self
            .max_increment_left_scroll_count
            .max(delta.left.scroll_count);
        self.max_increment_right_scroll_count = self
            .max_increment_right_scroll_count
            .max(delta.right.scroll_count);

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
            u_left_click_count: self.usage.left.click_count,
            u_right_click_count: self.usage.right.click_count,
            u_left_drag: self.usage.left.drag_duration,
            u_right_drag: self.usage.right.drag_duration,
            u_left_key_count: self.usage.left.key_count,
            u_right_key_count: self.usage.right.key_count,
            u_unclassified_key_count: self.usage.unclassified_key_count,
            u_left_combo_count: self.usage.left.modifier.same_hand_combo,
            u_right_combo_count: self.usage.right.modifier.same_hand_combo,
            u_unclassified_combo_count: self.usage.unclassified_key_combo,
            u_left_scroll_count: self.usage.left.scroll_count,
            u_right_scroll_count: self.usage.right.scroll_count,
            u_left_shift: self.usage.left.modifier.shift,
            u_left_ctrl: self.usage.left.modifier.ctrl,
            u_left_alt: self.usage.left.modifier.alt,
            u_left_meta: self.usage.left.modifier.meta,
            u_left_multi: self.usage.left.modifier.multi,
            u_right_shift: self.usage.right.modifier.shift,
            u_right_ctrl: self.usage.right.modifier.ctrl,
            u_right_alt: self.usage.right.modifier.alt,
            u_right_meta: self.usage.right.modifier.meta,
            u_right_multi: self.usage.right.modifier.multi,
            u_active: self.usage.active_duration,
            cb_left: self.credit.base.left.total(),
            cb_right: self.credit.base.right.total(),
            cb_unclassified: self.credit.base.unclassified_key,
            cx_left: self.credit.boost.left.total(),
            cx_right: self.credit.boost.right.total(),
            cx_unclassified: self.credit.boost.unclassified_key,
            max_increment_left_key_count: self.max_increment_left_key_count,
            max_increment_right_key_count: self.max_increment_right_key_count,
            max_increment_unclassified_key_count: self.max_increment_unclassified_key_count,
            max_increment_left_combo_count: self.max_increment_left_combo_count,
            max_increment_right_combo_count: self.max_increment_right_combo_count,
            max_increment_unclassified_combo_count: self.max_increment_unclassified_combo_count,
            max_increment_left_click_count: self.max_increment_left_click_count,
            max_increment_right_click_count: self.max_increment_right_click_count,
            max_increment_left_scroll_count: self.max_increment_left_scroll_count,
            max_increment_right_scroll_count: self.max_increment_right_scroll_count,
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
    delta.left.click_count != 0
        || delta.right.click_count != 0
        || delta.key_event_count() != 0
        || delta.left.scroll_count != 0
        || delta.right.scroll_count != 0
        || !delta.left.drag_duration.is_zero()
        || !delta.right.drag_duration.is_zero()
        || !delta.active_duration.is_zero()
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::model::{HandUsageDelta, ModifierUsageDelta};

    fn ts(millis: i64) -> Timestamp {
        Timestamp::UNIX_EPOCH + SignedDuration::from_millis(millis)
    }

    /// Increment spanning `[start_ms, end_ms)` with the given left key count.
    fn keys(start_ms: i64, end_ms: i64, key_count: u64) -> UsageIncrement {
        let delta = UsageDelta {
            left: HandUsageDelta {
                key_count,
                ..HandUsageDelta::default()
            },
            active_duration: Duration::from_millis((end_ms - start_ms) as u64),
            ..UsageDelta::default()
        };
        UsageIncrement::new(delta, ts(start_ms), ts(end_ms))
    }

    /// A credit increment with left-hand base and boost key credit set, so the
    /// total credit is exactly `base + boost`.
    fn credit(base: f64, boost: f64) -> CreditIncrement {
        let mut increment = CreditIncrement::default();
        increment.base.left.key = Credit::new(base);
        increment.boost.left.key = Credit::new(boost);
        increment
    }

    fn increment(start_ms: i64, end_ms: i64, mut delta: UsageDelta) -> UsageIncrement {
        if delta.active_duration.is_zero() {
            delta.active_duration = Duration::from_millis((end_ms - start_ms) as u64);
        }
        UsageIncrement::new(delta, ts(start_ms), ts(end_ms))
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
        assert_eq!(bucket.u_left_key_count, 10);
        assert_eq!(bucket.bucket_start, ts(0));
        assert_eq!(bucket.bucket_end, ts(3000));
        assert_eq!(bucket.cb_left.as_f64(), 3.5);
        assert_eq!(bucket.cx_left.as_f64(), 0.75);
        assert_eq!(bucket.observed_duration, Duration::from_millis(3000));
    }

    #[test]
    fn records_handed_raw_usage_and_compact_credit() {
        let mut builder = UsageCreditBucketBuilder::new();

        let first = increment(
            0,
            1000,
            UsageDelta {
                left: HandUsageDelta {
                    click_count: 2,
                    drag_duration: Duration::from_millis(50),
                    key_count: 2,
                    scroll_count: 5,
                    modifier: ModifierUsageDelta {
                        same_hand_combo: 3,
                        multi: Duration::from_millis(100),
                        ..ModifierUsageDelta::default()
                    },
                },
                right: HandUsageDelta {
                    click_count: 1,
                    key_count: 1,
                    ..HandUsageDelta::default()
                },
                ..UsageDelta::default()
            },
        );
        let second = increment(
            1000,
            2000,
            UsageDelta {
                left: HandUsageDelta {
                    key_count: 1,
                    ..HandUsageDelta::default()
                },
                right: HandUsageDelta {
                    drag_duration: Duration::from_millis(60),
                    key_count: 4,
                    scroll_count: 7,
                    modifier: ModifierUsageDelta {
                        same_hand_combo: 2,
                        multi: Duration::from_millis(200),
                        ..ModifierUsageDelta::default()
                    },
                    ..HandUsageDelta::default()
                },
                unclassified_key_count: 5,
                unclassified_key_combo: 8,
                ..UsageDelta::default()
            },
        );

        let mut first_credit = CreditIncrement::default();
        first_credit.base.left.key = Credit::new(1.0);
        first_credit.base.left.modifier = Credit::new(4.0);
        first_credit.base.right.key = Credit::new(2.0);
        first_credit.base.right.modifier = Credit::new(5.0);
        first_credit.base.unclassified_key = Credit::new(3.0);
        first_credit.boost = first_credit.base;

        let mut second_credit = CreditIncrement::default();
        second_credit.base.left.key = Credit::new(10.0);
        second_credit.base.left.modifier = Credit::new(40.0);
        second_credit.base.right.key = Credit::new(20.0);
        second_credit.base.right.modifier = Credit::new(50.0);
        second_credit.base.unclassified_key = Credit::new(30.0);
        second_credit.boost = second_credit.base;

        builder.push(&first, &first_credit);
        builder.push(&second, &second_credit);

        let bucket = builder.finish().expect("non-empty bucket");
        assert_eq!(bucket.u_left_click_count, 2);
        assert_eq!(bucket.u_right_click_count, 1);
        assert_eq!(bucket.u_left_drag, Duration::from_millis(50));
        assert_eq!(bucket.u_right_drag, Duration::from_millis(60));
        assert_eq!(bucket.u_left_key_count, 3);
        assert_eq!(bucket.u_right_key_count, 5);
        assert_eq!(bucket.u_unclassified_key_count, 5);
        assert_eq!(bucket.u_left_combo_count, 3);
        assert_eq!(bucket.u_right_combo_count, 2);
        assert_eq!(bucket.u_unclassified_combo_count, 8);
        assert_eq!(bucket.u_left_scroll_count, 5);
        assert_eq!(bucket.u_right_scroll_count, 7);
        assert_eq!(bucket.u_left_multi, Duration::from_millis(100));
        assert_eq!(bucket.u_right_multi, Duration::from_millis(200));
        assert_eq!(bucket.max_increment_left_key_count, 2);
        assert_eq!(bucket.max_increment_right_key_count, 4);
        assert_eq!(bucket.max_increment_unclassified_key_count, 5);
        assert_eq!(bucket.max_increment_left_combo_count, 3);
        assert_eq!(bucket.max_increment_right_combo_count, 2);
        assert_eq!(bucket.max_increment_unclassified_combo_count, 8);
        assert_eq!(bucket.max_increment_left_click_count, 2);
        assert_eq!(bucket.max_increment_right_click_count, 1);
        assert_eq!(bucket.max_increment_left_scroll_count, 5);
        assert_eq!(bucket.max_increment_right_scroll_count, 7);
        assert_eq!(bucket.cb_left, Credit::new(55.0));
        assert_eq!(bucket.cb_right, Credit::new(77.0));
        assert_eq!(bucket.cb_unclassified, Credit::new(33.0));
        assert_eq!(bucket.cx_left, Credit::new(55.0));
        assert_eq!(bucket.cx_right, Credit::new(77.0));
        assert_eq!(bucket.cx_unclassified, Credit::new(33.0));
    }

    #[test]
    fn finalizes_when_next_increment_crosses_target_end() {
        let mut builder = UsageCreditBucketBuilder::new();

        assert!(builder.push(&keys(0, 2000, 1), &credit(1.0, 0.0)).is_none());
        assert!(
            builder
                .push(&keys(2000, 4000, 1), &credit(1.0, 0.0))
                .is_none()
        );

        let first = builder
            .push(&keys(4000, 6000, 1), &credit(1.0, 0.0))
            .expect("first bucket finalized");
        assert_eq!(first.bucket_start, ts(0));
        assert_eq!(first.bucket_end, ts(4000));
        assert_eq!(first.increment_count, 2);

        let second = builder.finish().expect("second bucket");
        assert_eq!(second.bucket_start, ts(4000));
        assert_eq!(second.bucket_end, ts(6000));
        assert_eq!(second.increment_count, 1);
    }

    #[test]
    fn does_not_split_a_crossing_increment() {
        let mut builder = UsageCreditBucketBuilder::new();

        assert!(builder.push(&keys(0, 1000, 1), &credit(1.0, 0.0)).is_none());

        let first = builder
            .push(&keys(1000, 9000, 4), &credit(2.0, 0.0))
            .expect("first bucket finalized");
        assert_eq!(first.increment_count, 1);
        assert_eq!(first.u_left_key_count, 1);

        let second = builder.finish().expect("second bucket");
        assert_eq!(second.bucket_start, ts(1000));
        assert_eq!(second.bucket_end, ts(9000));
        assert_eq!(second.u_left_key_count, 4);
    }

    #[test]
    fn computes_max_and_squared_credit_stats() {
        let mut builder = UsageCreditBucketBuilder::new();

        builder.push(&keys(0, 1000, 2), &credit(1.0, 0.0));
        builder.push(&keys(1000, 2000, 7), &credit(2.0, 1.0));
        builder.push(&keys(2000, 3000, 4), &credit(0.0, 0.5));

        let bucket = builder.finish().expect("non-empty bucket");
        assert_eq!(bucket.max_increment_left_key_count, 7);
        assert_eq!(bucket.max_increment_total_credit, Credit::new(3.0));
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
        assert!(builder.finish().is_none());
    }

    #[test]
    fn skips_no_op_buckets() {
        let mut builder = UsageCreditBucketBuilder::new();

        let idle = UsageIncrement::new(UsageDelta::default(), ts(0), ts(1000));
        assert!(builder.push(&idle, &CreditIncrement::default()).is_none());
        assert!(builder.finish().is_none());
    }
}
