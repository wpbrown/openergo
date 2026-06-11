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
use std::future::Future;
use std::num::NonZeroUsize;

/// Persisted snapshot of the most recently published per-kind ratios and
/// the current escalation level for each kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CreditUtilizationState {
    last_published: Utilization,
    escalation_level: EscalationLevels,
}

impl CreditUtilizationState {
    /// Most recently published per-kind rounded ratios.
    pub fn last_published(&self) -> &Utilization {
        &self.last_published
    }
}

/// Per-kind rounded `credit / limit` ratios (2 decimal places).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Utilization {
    pub rest: f64,
    #[serde(rename = "break")]
    pub breaks: f64,
    pub day: f64,
}

/// Per-kind escalation level. `0` means "not currently over the limit"
/// (or a reset has occurred); `1..=5` represent the discrete escalation
/// steps at 110%, 120%, ..., 150%.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct EscalationLevels {
    pub rest: u8,
    #[serde(rename = "break")]
    pub breaks: u8,
    pub day: u8,
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
    let mut state = initial;

    loop {
        let current = compute_ratios(&sources, &limits);
        let (new_state, events) = diff(current, &state);

        if new_state != state {
            let _ = state_producer.update(|s| *s = new_state);
            state = new_state;
        }

        for ev in events {
            let _ = event_producer.try_send(ev);
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

/// Read the current per-kind ratios from the input watches, rounded to
/// 2 decimal places. A non-positive limit yields a ratio of 0.0
/// defensively (config validation rejects this in Phase 0).
fn compute_ratios(sources: &AllUsageConsumer, limits: &CreditLimitConsumer) -> Utilization {
    sources.view(|_all, rest, breaks, day| {
        limits.view(|lim| Utilization {
            rest: round2(ratio(rest.credit().total(), lim.rest)),
            breaks: round2(ratio(breaks.credit().total(), lim.breaks)),
            day: round2(ratio(day.credit().total(), lim.day)),
        })
    })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Pure per-tick diff. Given the freshly computed (rounded) ratios and
/// the previous state, returns the new state plus any events that should
/// be emitted (in `[Rest, Breaks, Day]` order).
fn diff(
    current: Utilization,
    prev: &CreditUtilizationState,
) -> (CreditUtilizationState, Vec<CreditEvent>) {
    let mut events = Vec::new();
    let mut new_state = *prev;

    let (rest_level, rest_ev) = diff_kind(
        CreditKind::Rest,
        current.rest,
        prev.last_published.rest,
        prev.escalation_level.rest,
    );
    new_state.escalation_level.rest = rest_level;
    new_state.last_published.rest = current.rest;
    if let Some(ev) = rest_ev {
        events.push(ev);
    }

    let (breaks_level, breaks_ev) = diff_kind(
        CreditKind::Breaks,
        current.breaks,
        prev.last_published.breaks,
        prev.escalation_level.breaks,
    );
    new_state.escalation_level.breaks = breaks_level;
    new_state.last_published.breaks = current.breaks;
    if let Some(ev) = breaks_ev {
        events.push(ev);
    }

    let (day_level, day_ev) = diff_kind(
        CreditKind::Day,
        current.day,
        prev.last_published.day,
        prev.escalation_level.day,
    );
    new_state.escalation_level.day = day_level;
    new_state.last_published.day = current.day;
    if let Some(ev) = day_ev {
        events.push(ev);
    }

    (new_state, events)
}

/// Per-kind diff rules. Returns `(new_level, optional_event)`.
fn diff_kind(kind: CreditKind, current: f64, last: f64, level: u8) -> (u8, Option<CreditEvent>) {
    if current < last {
        (0, Some(CreditEvent::Reset { kind }))
    } else if last < 1.0 && current >= 1.0 {
        (0, Some(CreditEvent::Reached { kind }))
    } else if level < MAX_ESCALATION_LEVEL
        && current >= 1.0 + ESCALATION_STEP * f64::from(level + 1)
    {
        let new_level = level + 1;
        (
            new_level,
            Some(CreditEvent::Escalation {
                kind,
                level: new_level,
            }),
        )
    } else {
        (level, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(rest: f64, level: u8) -> CreditUtilizationState {
        CreditUtilizationState {
            last_published: Utilization {
                rest,
                breaks: 0.0,
                day: 0.0,
            },
            escalation_level: EscalationLevels {
                rest: level,
                breaks: 0,
                day: 0,
            },
        }
    }

    fn ratios(rest: f64) -> Utilization {
        Utilization {
            rest,
            breaks: 0.0,
            day: 0.0,
        }
    }

    #[test]
    fn rising_past_100_fires_reached_not_escalation() {
        let prev = state(0.5, 0);
        let (new, evs) = diff(ratios(1.0), &prev);
        assert_eq!(
            evs,
            vec![CreditEvent::Reached {
                kind: CreditKind::Rest
            }]
        );
        assert_eq!(new.escalation_level.rest, 0);
        assert_eq!(new.last_published.rest, 1.0);
    }

    #[test]
    fn rising_through_steps_fires_escalations_1_through_5() {
        let mut state = state(1.0, 0);
        let steps = [(1.10, 1u8), (1.20, 2), (1.30, 3), (1.40, 4), (1.50, 5)];
        for (current, expected_level) in steps {
            let (new, evs) = diff(ratios(current), &state);
            assert_eq!(
                evs,
                vec![CreditEvent::Escalation {
                    kind: CreditKind::Rest,
                    level: expected_level,
                }],
                "step at current={current}"
            );
            assert_eq!(new.escalation_level.rest, expected_level);
            state = new;
        }
    }

    #[test]
    fn stuck_at_200_does_not_keep_firing() {
        // Already at the cap with last=2.0 level=5; staying at 2.0 emits
        // nothing.
        let prev = state(2.0, 5);
        let (new, evs) = diff(ratios(2.0), &prev);
        assert!(evs.is_empty());
        assert_eq!(new.escalation_level.rest, 5);

        // Even if we arrived at level 5 with last=1.5 and ratio jumps to
        // 2.0, no further escalation is possible.
        let prev = state(1.5, 5);
        let (new, evs) = diff(ratios(2.0), &prev);
        assert!(evs.is_empty());
        assert_eq!(new.escalation_level.rest, 5);
        assert_eq!(new.last_published.rest, 2.0);
    }

    #[test]
    fn decrease_anywhere_fires_reset() {
        for &(last, level, current) in &[
            (1.50f64, 3u8, 1.40f64),
            (2.00, 5, 1.99),
            (1.00, 0, 0.50),
            (0.80, 0, 0.70),
        ] {
            let prev = state(last, level);
            let (new, evs) = diff(ratios(current), &prev);
            assert_eq!(
                evs,
                vec![CreditEvent::Reset {
                    kind: CreditKind::Rest
                }],
                "decrease from last={last} level={level} to {current}"
            );
            assert_eq!(new.escalation_level.rest, 0);
            assert_eq!(new.last_published.rest, current);
        }
    }

    #[test]
    fn reset_followed_by_recross_fires_reached_again() {
        // Start above the limit at level 3.
        let prev = state(1.30, 3);
        // Drop below 1.0 -> Reset.
        let (after_reset, evs) = diff(ratios(0.50), &prev);
        assert_eq!(
            evs,
            vec![CreditEvent::Reset {
                kind: CreditKind::Rest
            }]
        );
        assert_eq!(after_reset.escalation_level.rest, 0);

        // Re-cross 1.0 -> Reached again.
        let (after_recross, evs) = diff(ratios(1.0), &after_reset);
        assert_eq!(
            evs,
            vec![CreditEvent::Reached {
                kind: CreditKind::Rest
            }]
        );
        assert_eq!(after_recross.escalation_level.rest, 0);
    }

    #[test]
    fn limit_change_pushing_stable_credit_over_100_fires_reached() {
        // Previously at 0.90 (under limit). A limit reduction (computed
        // outside the diff function) presents the new ratio as 1.05.
        let prev = state(0.90, 0);
        let (new, evs) = diff(ratios(1.05), &prev);
        assert_eq!(
            evs,
            vec![CreditEvent::Reached {
                kind: CreditKind::Rest
            }]
        );
        assert_eq!(new.escalation_level.rest, 0);
        assert_eq!(new.last_published.rest, 1.05);
    }
}
