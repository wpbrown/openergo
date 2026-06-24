use crate::credit::limit::CreditLimitConsumer;
use crate::usage::AllUsageConsumer;
use bachelor::broadcast::spmc::{
    SpmcBroadcastConsumer, SpmcBroadcastProducer, SpmcBroadcastSource, broadcast,
};
use bachelor::error::Closed;
use bachelor::watch::{MpmcWatchRefConsumer, MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch};
use futures::future::{Either, select};
use serde::{Deserialize, Serialize};
use shared::model::ratio;
use smallvec::SmallVec;
use std::future::Future;
use std::num::NonZeroUsize;

/// Persisted snapshot of the most recently published per-kind raw ratios.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CreditUtilizationState {
    last_published: Utilization,
}

impl CreditUtilizationState {
    /// Most recently published per-kind raw ratios.
    pub fn last_published(&self) -> &Utilization {
        &self.last_published
    }
}

/// Per-kind raw `credit / limit` ratios.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Utilization {
    pub rest: f64,
    #[serde(rename = "break")]
    pub breaks: f64,
    pub day: f64,
}

/// Discriminator for the three credit kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditKind {
    Rest,
    Breaks,
    Day,
}

impl CreditKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CreditKind::Rest => "rest",
            CreditKind::Breaks => "breaks",
            CreditKind::Day => "day",
        }
    }
}

/// Discrete event emitted by the utilization driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditEvent {
    Reached { kind: CreditKind },
    Escalation { kind: CreditKind, level: u8 },
    Reset { kind: CreditKind },
}

/// Source-side handle to the utilization watch. Cloneable so multiple
/// consumers can subscribe.
#[derive(Clone)]
pub struct CreditUtilizationSource {
    inner: MpmcWatchRefSource<CreditUtilizationState>,
}

impl CreditUtilizationSource {
    pub fn subscribe_forward(&self) -> CreditUtilizationConsumer {
        CreditUtilizationConsumer {
            inner: self.inner.subscribe_forward(),
        }
    }
}

pub struct CreditUtilizationConsumer {
    inner: MpmcWatchRefConsumer<CreditUtilizationState>,
}

impl CreditUtilizationConsumer {
    pub fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin {
        self.inner.changed()
    }

    pub fn view<R>(&self, f: impl FnOnce(&CreditUtilizationState) -> R) -> R {
        self.inner.view(f)
    }
}

impl crate::watch_mux::FiniteChanges for CreditUtilizationConsumer {
    fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin + '_ {
        CreditUtilizationConsumer::changed(self)
    }
}

/// Source-side handle to the credit-event broadcast. Cloneable so
/// multiple consumers can independently subscribe.
#[derive(Clone)]
pub struct CreditEventSource {
    inner: SpmcBroadcastSource<CreditEvent>,
}

impl CreditEventSource {
    pub fn subscribe(&self) -> CreditEventConsumer {
        CreditEventConsumer {
            inner: self.inner.subscribe(),
        }
    }
}

pub struct CreditEventConsumer {
    inner: SpmcBroadcastConsumer<CreditEvent>,
}

impl CreditEventConsumer {
    pub fn try_recv(&self) -> Result<Option<CreditEvent>, Closed> {
        self.inner.try_recv()
    }

    pub async fn recv(&mut self) -> Result<CreditEvent, Closed> {
        self.inner.recv_ref(|ev| *ev).await
    }
}

impl crate::watch_mux::FiniteChanges for CreditEventConsumer {
    fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin + '_ {
        self.inner.ready()
    }
}

/// Maximum escalation step (corresponds to 150% utilization).
const MAX_ESCALATION_LEVEL: u8 = 5;
/// Per-step escalation increment (10%).
const ESCALATION_STEP: f64 = 0.10;
const ESCALATION_STEP_SCALE: f64 = 1.0 / ESCALATION_STEP;
type CreditEvents = SmallVec<[CreditEvent; 3]>;
/// Broadcast channel capacity for credit events. Internal consumers are
/// expected to poll immediately; 16 is generous.
const EVENT_BROADCAST_CAPACITY: NonZeroUsize =
    NonZeroUsize::new(16).expect("event broadcast capacity must be non-zero");

/// Construct the utilization watch + event broadcast and return the driver
/// future. The driver exits when either finite input closes.
pub fn create(
    initial: CreditUtilizationState,
    sources: AllUsageConsumer,
    limits: CreditLimitConsumer,
) -> (
    CreditUtilizationSource,
    CreditEventSource,
    impl Future<Output = ()> + use<>,
) {
    let (state_producer, state_source) = mpmc_watch(initial);
    let (event_producer, event_source) = broadcast::<CreditEvent>(EVENT_BROADCAST_CAPACITY);

    let driver = run(initial, sources, limits, state_producer, event_producer);

    (
        CreditUtilizationSource {
            inner: state_source,
        },
        CreditEventSource {
            inner: event_source,
        },
        driver,
    )
}

async fn run(
    initial: CreditUtilizationState,
    mut sources: AllUsageConsumer,
    mut limits: CreditLimitConsumer,
    state_producer: MpmcWatchRefProducer<CreditUtilizationState>,
    event_producer: SpmcBroadcastProducer<CreditEvent>,
) {
    let mut last_published = initial.last_published;
    let mut last_publish_bucket = publish_bucket(last_published);

    loop {
        let current = compute_ratios(&sources, &limits);
        let current_publish_bucket = publish_bucket(current);

        if current_publish_bucket != last_publish_bucket {
            let events = report_events(current, last_published);
            let new_state = CreditUtilizationState {
                last_published: current,
            };
            let _ = state_producer.update(|s| *s = new_state);
            last_published = current;
            last_publish_bucket = current_publish_bucket;

            for ev in events {
                let _ = event_producer.try_send(ev);
            }
        }

        let s_changed = sources.changed();
        let l_changed = limits.changed();

        match select(s_changed, l_changed).await {
            // Sources closed: nothing further to derive; exit so any
            // downstream `CreditEventSource` consumer also unblocks.
            Either::Left((Err(Closed), _)) => return,
            // Limits closed: same reasoning.
            Either::Right((Err(Closed), _)) => return,
            // Either input changed (Ok): recompute next iteration.
            Either::Left((Ok(_), _)) | Either::Right((Ok(()), _)) => {}
        }
    }
}

/// Read the current per-kind raw ratios from the input watches.
/// A non-positive limit yields a ratio of 0.0
/// defensively (config validation rejects this in Phase 0).
fn compute_ratios(sources: &AllUsageConsumer, limits: &CreditLimitConsumer) -> Utilization {
    sources.view(|_all, rest, breaks, day| {
        limits.view(|lim| Utilization {
            rest: ratio(rest.credit().total(), lim.rest),
            breaks: ratio(breaks.credit().total(), lim.breaks),
            day: ratio(day.credit().total(), lim.day),
        })
    })
}

fn publish_bucket(value: Utilization) -> Utilization {
    Utilization {
        rest: floor_tenth_percent(value.rest),
        breaks: floor_tenth_percent(value.breaks),
        day: floor_tenth_percent(value.day),
    }
}

fn floor_tenth_percent(value: f64) -> f64 {
    (value * 1000.0).floor() / 1000.0
}

/// Events for a reportable utilization change, emitted in `[Rest, Breaks, Day]` order.
fn report_events(current: Utilization, last: Utilization) -> CreditEvents {
    let mut events = SmallVec::new();

    let rest_ev = report_event_for_kind(CreditKind::Rest, current.rest, last.rest);
    if let Some(ev) = rest_ev {
        events.push(ev);
    }

    let breaks_ev = report_event_for_kind(CreditKind::Breaks, current.breaks, last.breaks);
    if let Some(ev) = breaks_ev {
        events.push(ev);
    }

    let day_ev = report_event_for_kind(CreditKind::Day, current.day, last.day);
    if let Some(ev) = day_ev {
        events.push(ev);
    }

    events
}

fn report_event_for_kind(kind: CreditKind, current: f64, last: f64) -> Option<CreditEvent> {
    let current_level = escalation_level(current);
    let last_level = escalation_level(last);

    if current < last {
        Some(CreditEvent::Reset { kind })
    } else if current_level > last_level {
        Some(CreditEvent::Escalation {
            kind,
            level: current_level,
        })
    } else if last < 1.0 && current >= 1.0 {
        Some(CreditEvent::Reached { kind })
    } else {
        None
    }
}

fn escalation_level(current: f64) -> u8 {
    ((current * ESCALATION_STEP_SCALE - ESCALATION_STEP_SCALE).floor() as u8)
        .min(MAX_ESCALATION_LEVEL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ratios(rest: f64) -> Utilization {
        Utilization {
            rest,
            breaks: 0.0,
            day: 0.0,
        }
    }

    #[test]
    fn publish_bucket_floors_to_tenth_percent() {
        assert_eq!(publish_bucket(ratios(0.9999)).rest, 0.999);
        assert_eq!(publish_bucket(ratios(1.0001)).rest, 1.0);
        assert_eq!(publish_bucket(ratios(1.1099)).rest, 1.109);
        assert_eq!(publish_bucket(ratios(1.1100)).rest, 1.11);
    }

    #[test]
    fn rising_past_100_fires_reached_not_escalation() {
        let evs = report_events(ratios(1.0), ratios(0.5));
        assert_eq!(
            evs.as_slice(),
            &[CreditEvent::Reached {
                kind: CreditKind::Rest
            }]
        );
    }

    #[test]
    fn rising_through_steps_fires_escalations_1_through_5() {
        let mut last = ratios(1.0);
        let steps = [(1.10, 1u8), (1.20, 2), (1.30, 3), (1.40, 4), (1.50, 5)];
        for (current, expected_level) in steps {
            let current = ratios(current);
            let evs = report_events(current, last);
            assert_eq!(
                evs.as_slice(),
                &[CreditEvent::Escalation {
                    kind: CreditKind::Rest,
                    level: expected_level,
                }],
                "step at current={}",
                current.rest
            );
            last = current;
        }
    }

    #[test]
    fn jumping_across_escalations_fires_latest_level_only() {
        let evs = report_events(ratios(1.31), ratios(1.09));
        assert_eq!(
            evs.as_slice(),
            &[CreditEvent::Escalation {
                kind: CreditKind::Rest,
                level: 3,
            }]
        );

        let evs = report_events(ratios(1.31), ratios(0.99));
        assert_eq!(
            evs.as_slice(),
            &[CreditEvent::Escalation {
                kind: CreditKind::Rest,
                level: 3,
            }]
        );
    }

    #[test]
    fn stuck_at_200_does_not_keep_firing() {
        let evs = report_events(ratios(2.0), ratios(2.0));
        assert!(evs.is_empty());
        assert_eq!(escalation_level(2.0), 5);

        let evs = report_events(ratios(2.0), ratios(1.5));
        assert!(evs.is_empty());
    }

    #[test]
    fn decrease_anywhere_fires_reset() {
        for &(last, current) in &[(1.50f64, 1.40f64), (2.00, 1.99), (1.00, 0.50), (0.80, 0.70)] {
            let evs = report_events(ratios(current), ratios(last));
            assert_eq!(
                evs.as_slice(),
                &[CreditEvent::Reset {
                    kind: CreditKind::Rest
                }],
                "decrease from last={last} to {current}"
            );
        }
    }

    #[test]
    fn reset_followed_by_recross_fires_reached_again() {
        let evs = report_events(ratios(0.50), ratios(1.30));
        assert_eq!(
            evs.as_slice(),
            &[CreditEvent::Reset {
                kind: CreditKind::Rest
            }]
        );

        let evs = report_events(ratios(1.0), ratios(0.50));
        assert_eq!(
            evs.as_slice(),
            &[CreditEvent::Reached {
                kind: CreditKind::Rest
            }]
        );
    }

    #[test]
    fn limit_change_pushing_stable_credit_over_100_fires_reached() {
        let evs = report_events(ratios(1.05), ratios(0.90));
        assert_eq!(
            evs.as_slice(),
            &[CreditEvent::Reached {
                kind: CreditKind::Rest
            }]
        );
    }
}
