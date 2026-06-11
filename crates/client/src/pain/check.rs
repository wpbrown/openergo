use super::PainConsumer;
use crate::activity::ActivityStateConsumer;
use crate::credit::utilization::CreditEventConsumer;
use crate::integration::{AnalogIn, AnalogOutProducer};
use crate::pain::check::heuristics::PainStateUpdateEffect;
use crate::watch_mux::{WatchMux, define_watch_mux};
use jiff::Timestamp;
use rootcause::prelude::*;
use tracing::{debug, info, trace};

define_watch_mux! {
    pub struct PainCheckInputs;
    pub flags PainCheckInput;
    pain: PainConsumer => PAIN,
    credit_events: CreditEventConsumer => CREDIT_EVENTS,
    activity: ActivityStateConsumer => ACTIVITY,
    acknowledge: Option<AnalogIn> => ACKNOWLEDGE,
}

pub async fn run(inputs: PainCheckInputs, output: Option<AnalogOutProducer>) -> Result<(), Report> {
    let now = Timestamp::now();
    let last_confirmed_at = inputs
        .pain
        .view(|pain, _| heuristics::last_confirmed_at(pain, now));
    let activity_total = inputs.activity.view(|activity| activity.total());
    let mut state = heuristics::PainCheckState::new(last_confirmed_at, now, activity_total);
    debug!(
        indicator_configured = output.is_some(),
        activity_total_secs = activity_total.as_secs(),
        confirmation_age_secs = now
            .duration_since(last_confirmed_at)
            .unsigned_abs()
            .as_secs(),
        "pain check driver started"
    );
    update_indicator(output.as_ref(), state.pain_input_needed());

    let mut inputs = WatchMux::new(inputs);

    while let Ok(input) = inputs.changed().await {
        match input {
            PainCheckInput::PAIN => confirm_pain(&mut state, output.as_ref(), "pain_input"),
            PainCheckInput::ACKNOWLEDGE => confirm_pain(&mut state, output.as_ref(), "acknowledge"),
            PainCheckInput::CREDIT_EVENTS => {
                let mut pain_input_needed = PainStateUpdateEffect::Unchanged;
                while let Ok(Some(event)) = inputs.get_mut().credit_events.try_recv() {
                    pain_input_needed |= state.observe_credit_event(Timestamp::now(), event);
                }
                if pain_input_needed.changed() {
                    update_indicator_changed(
                        output.as_ref(),
                        state.pain_input_needed(),
                        "credit_event",
                    );
                }
            }
            PainCheckInput::ACTIVITY => {
                let activity_total = inputs.get().activity.view(|activity| activity.total());
                if state
                    .observe_activity(Timestamp::now(), activity_total)
                    .changed()
                {
                    update_indicator_changed(
                        output.as_ref(),
                        state.pain_input_needed(),
                        "activity",
                    );
                }
            }
            _ => {}
        }
    }

    Ok(())
}

fn confirm_pain(
    state: &mut heuristics::PainCheckState,
    output: Option<&AnalogOutProducer>,
    source: &'static str,
) {
    if state.confirm(Timestamp::now()).changed() {
        debug!(source, "pain confirmation cleared update request");
        update_indicator_changed(output, state.pain_input_needed(), source);
    } else {
        trace!(
            source,
            "pain confirmation refreshed without indicator change"
        );
    }
}

fn update_indicator(output: Option<&AnalogOutProducer>, pain_input_needed: bool) {
    if let Some(indicator) = output {
        let _ = indicator.update(if pain_input_needed { 1.0 } else { 0.0 });
    }
}

fn update_indicator_changed(
    output: Option<&AnalogOutProducer>,
    pain_input_needed: bool,
    reason: &'static str,
) {
    if output.is_some() {
        info!(pain_input_needed, reason, "pain check indicator changed");
    } else {
        debug!(
            pain_input_needed,
            reason, "pain check indicator changed without configured output"
        );
    }
    update_indicator(output, pain_input_needed);
}

mod heuristics {
    use super::super::PainState;
    use crate::credit::utilization::{CreditEvent, CreditKind};
    use jiff::Timestamp;
    use std::ops::{BitOr, BitOrAssign};
    use std::time::Duration;
    use tracing::{debug, trace};

    const REST_REACHED_STALE_AFTER: Duration = Duration::from_secs(5 * 60);
    const REST_ESCALATION_STALE_AFTER: Duration = Duration::from_secs(2 * 60);
    const REST_HIGH_ESCALATION_STALE_AFTER: Duration = Duration::from_secs(60);
    const BREAK_REACHED_STALE_AFTER: Duration = Duration::from_secs(10 * 60);
    const BREAK_ESCALATION_STALE_AFTER: Duration = Duration::from_secs(5 * 60);
    const DAY_REACHED_STALE_AFTER: Duration = Duration::from_secs(45 * 60);
    const DAY_ESCALATION_STALE_AFTER: Duration = Duration::from_secs(30 * 60);
    const IDLE_RESUME_GAP: Duration = Duration::from_secs(30 * 60);
    const POST_IDLE_STALE_AFTER: Duration = Duration::from_secs(3 * 60);
    const SUSTAINED_ACTIVITY_DELTA: Duration = Duration::from_secs(30 * 60);
    const SUSTAINED_ACTIVITY_STALE_AFTER: Duration = Duration::from_secs(30 * 60);

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum PainStateUpdateEffect {
        Changed,
        Unchanged,
    }

    impl PainStateUpdateEffect {
        pub(super) fn changed(self) -> bool {
            matches!(self, Self::Changed)
        }

        fn if_changed(changed: bool) -> Self {
            if changed {
                Self::Changed
            } else {
                Self::Unchanged
            }
        }
    }

    impl BitOr for PainStateUpdateEffect {
        type Output = Self;

        fn bitor(self, rhs: Self) -> Self::Output {
            if self.changed() || rhs.changed() {
                Self::Changed
            } else {
                Self::Unchanged
            }
        }
    }

    impl BitOrAssign for PainStateUpdateEffect {
        fn bitor_assign(&mut self, rhs: Self) {
            *self = *self | rhs;
        }
    }

    pub(super) struct PainCheckState {
        pain_input_needed: bool,
        last_confirmed_at: Timestamp,
        last_activity_seen_at: Timestamp,
        last_activity_total: Duration,
        activity_total_at_last_confirmation: Duration,
    }

    impl PainCheckState {
        pub(super) fn new(
            last_confirmed_at: Timestamp,
            now: Timestamp,
            activity_total: Duration,
        ) -> Self {
            Self {
                pain_input_needed: false,
                last_confirmed_at,
                last_activity_seen_at: now,
                last_activity_total: activity_total,
                activity_total_at_last_confirmation: activity_total,
            }
        }

        pub(super) fn pain_input_needed(&self) -> bool {
            self.pain_input_needed
        }

        pub(super) fn confirm(&mut self, now: Timestamp) -> PainStateUpdateEffect {
            let confirmation_age = now.duration_since(self.last_confirmed_at).unsigned_abs();
            let pain_input_was_needed = self.pain_input_needed;
            self.last_confirmed_at = now;
            self.pain_input_needed = false;
            self.activity_total_at_last_confirmation = self.last_activity_total;
            trace!(
                confirmation_age_secs = confirmation_age.as_secs(),
                pain_input_was_needed, "pain reading confirmed"
            );
            PainStateUpdateEffect::if_changed(pain_input_was_needed)
        }

        pub(super) fn observe_credit_event(
            &mut self,
            now: Timestamp,
            event: CreditEvent,
        ) -> PainStateUpdateEffect {
            match event {
                CreditEvent::Reached { kind } => {
                    let stale_after = match kind {
                        CreditKind::Rest => REST_REACHED_STALE_AFTER,
                        CreditKind::Breaks => BREAK_REACHED_STALE_AFTER,
                        CreditKind::Day => DAY_REACHED_STALE_AFTER,
                    };
                    debug!(
                        kind = kind.as_str(),
                        stale_after_secs = stale_after.as_secs(),
                        "credit limit reached; checking pain confirmation freshness"
                    );
                    self.request_if_stale(now, stale_after, "credit_reached")
                }
                CreditEvent::Escalation { kind, level } => {
                    let stale_after = match kind {
                        CreditKind::Rest if level >= 3 => REST_HIGH_ESCALATION_STALE_AFTER,
                        CreditKind::Rest => REST_ESCALATION_STALE_AFTER,
                        CreditKind::Breaks => BREAK_ESCALATION_STALE_AFTER,
                        CreditKind::Day => DAY_ESCALATION_STALE_AFTER,
                    };
                    debug!(
                        kind = kind.as_str(),
                        level,
                        stale_after_secs = stale_after.as_secs(),
                        "credit escalation; checking pain confirmation freshness"
                    );
                    self.request_if_stale(now, stale_after, "credit_escalation")
                }
                CreditEvent::Reset { kind } => {
                    trace!(kind = kind.as_str(), "credit reset ignored by pain check");
                    PainStateUpdateEffect::Unchanged
                }
            }
        }

        pub(super) fn observe_activity(
            &mut self,
            now: Timestamp,
            activity_total: Duration,
        ) -> PainStateUpdateEffect {
            let increased = activity_total > self.last_activity_total;
            let idle_gap = now
                .duration_since(self.last_activity_seen_at)
                .unsigned_abs();

            self.last_activity_seen_at = now;
            self.last_activity_total = activity_total;

            if increased && idle_gap >= IDLE_RESUME_GAP {
                debug!(
                    idle_gap_secs = idle_gap.as_secs(),
                    stale_after_secs = POST_IDLE_STALE_AFTER.as_secs(),
                    "activity resumed after idle; checking pain confirmation freshness"
                );
                self.request_if_stale(now, POST_IDLE_STALE_AFTER, "activity_resumed_after_idle")
            } else {
                self.observe_sustained_activity(now)
            }
        }

        fn observe_sustained_activity(&mut self, now: Timestamp) -> PainStateUpdateEffect {
            let activity_since_confirmation = self
                .last_activity_total
                .saturating_sub(self.activity_total_at_last_confirmation);
            if activity_since_confirmation < SUSTAINED_ACTIVITY_DELTA {
                return PainStateUpdateEffect::Unchanged;
            }

            debug!(
                activity_since_confirmation_secs = activity_since_confirmation.as_secs(),
                stale_after_secs = SUSTAINED_ACTIVITY_STALE_AFTER.as_secs(),
                "sustained activity; checking pain confirmation freshness"
            );
            self.request_if_stale(now, SUSTAINED_ACTIVITY_STALE_AFTER, "sustained_activity")
        }

        fn request_if_stale(
            &mut self,
            now: Timestamp,
            stale_after: Duration,
            reason: &'static str,
        ) -> PainStateUpdateEffect {
            let confirmation_age = now.duration_since(self.last_confirmed_at).unsigned_abs();
            if self.pain_input_needed {
                trace!(
                    reason,
                    confirmation_age_secs = confirmation_age.as_secs(),
                    stale_after_secs = stale_after.as_secs(),
                    "pain update request already pending"
                );
                return PainStateUpdateEffect::Unchanged;
            }

            if confirmation_age < stale_after {
                trace!(
                    reason,
                    confirmation_age_secs = confirmation_age.as_secs(),
                    stale_after_secs = stale_after.as_secs(),
                    "pain confirmation is fresh enough"
                );
                return PainStateUpdateEffect::Unchanged;
            }

            debug!(
                reason,
                confirmation_age_secs = confirmation_age.as_secs(),
                stale_after_secs = stale_after.as_secs(),
                "pain confirmation is stale; requesting update"
            );
            self.pain_input_needed = true;
            PainStateUpdateEffect::Changed
        }
    }

    pub(super) fn last_confirmed_at(pain: &PainState, fallback: Timestamp) -> Timestamp {
        pain.entries
            .iter()
            .map(|(_, entry)| entry.last_updated())
            .max()
            .unwrap_or(fallback)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn ts(seconds: u64) -> Timestamp {
            Timestamp::UNIX_EPOCH + Duration::from_secs(seconds)
        }

        #[test]
        fn rest_reached_requests_when_confirmation_is_old() {
            let mut state = PainCheckState::new(ts(0), ts(0), Duration::ZERO);

            assert_eq!(
                state.observe_credit_event(
                    ts(5 * 60),
                    CreditEvent::Reached {
                        kind: CreditKind::Rest,
                    },
                ),
                PainStateUpdateEffect::Changed
            );
            assert!(state.pain_input_needed());
        }

        #[test]
        fn rest_reached_is_discarded_when_confirmation_is_fresh() {
            let mut state = PainCheckState::new(ts(0), ts(0), Duration::ZERO);

            assert_eq!(
                state.observe_credit_event(
                    ts(4 * 60),
                    CreditEvent::Reached {
                        kind: CreditKind::Rest,
                    },
                ),
                PainStateUpdateEffect::Unchanged
            );
            assert!(!state.pain_input_needed());
        }

        #[test]
        fn confirmation_clears_request_and_refreshes_freshness() {
            let mut state = PainCheckState::new(ts(0), ts(0), Duration::ZERO);
            assert_eq!(
                state.observe_credit_event(
                    ts(5 * 60),
                    CreditEvent::Reached {
                        kind: CreditKind::Rest,
                    },
                ),
                PainStateUpdateEffect::Changed
            );

            assert_eq!(
                state.confirm(ts(5 * 60 + 30)),
                PainStateUpdateEffect::Changed
            );
            assert!(!state.pain_input_needed());
            assert_eq!(
                state.observe_credit_event(
                    ts(9 * 60),
                    CreditEvent::Reached {
                        kind: CreditKind::Rest,
                    },
                ),
                PainStateUpdateEffect::Unchanged
            );
        }

        #[test]
        fn high_rest_escalation_uses_tighter_freshness() {
            let mut low = PainCheckState::new(ts(0), ts(0), Duration::ZERO);
            let mut high = PainCheckState::new(ts(0), ts(0), Duration::ZERO);

            assert_eq!(
                low.observe_credit_event(
                    ts(61),
                    CreditEvent::Escalation {
                        kind: CreditKind::Rest,
                        level: 1,
                    },
                ),
                PainStateUpdateEffect::Unchanged
            );
            assert_eq!(
                high.observe_credit_event(
                    ts(61),
                    CreditEvent::Escalation {
                        kind: CreditKind::Rest,
                        level: 3,
                    },
                ),
                PainStateUpdateEffect::Changed
            );
        }

        #[test]
        fn activity_resume_after_idle_requests_when_confirmation_is_old() {
            let mut state = PainCheckState::new(ts(0), ts(0), Duration::from_secs(10));

            assert_eq!(
                state.observe_activity(ts(1), Duration::from_secs(11)),
                PainStateUpdateEffect::Unchanged
            );
            assert_eq!(
                state.observe_activity(ts(1 + 30 * 60), Duration::from_secs(12)),
                PainStateUpdateEffect::Changed
            );
            assert!(state.pain_input_needed());
        }

        #[test]
        fn activity_resume_after_idle_is_discarded_when_confirmation_is_fresh() {
            let mut state = PainCheckState::new(ts(0), ts(0), Duration::from_secs(10));

            assert_eq!(
                state.observe_activity(ts(1), Duration::from_secs(11)),
                PainStateUpdateEffect::Unchanged
            );
            assert_eq!(state.confirm(ts(29 * 60)), PainStateUpdateEffect::Unchanged);
            assert_eq!(
                state.observe_activity(ts(1 + 30 * 60), Duration::from_secs(12)),
                PainStateUpdateEffect::Unchanged
            );
            assert!(!state.pain_input_needed());
        }

        #[test]
        fn sustained_activity_requests_when_confirmation_is_old() {
            let mut state = PainCheckState::new(ts(0), ts(0), Duration::ZERO);

            assert_eq!(
                state.observe_activity(ts(30 * 60), Duration::from_secs(30 * 60),),
                PainStateUpdateEffect::Changed
            );
            assert!(state.pain_input_needed());
        }

        #[test]
        fn sustained_activity_waits_for_activity_delta() {
            let mut state = PainCheckState::new(ts(0), ts(0), Duration::ZERO);

            assert_eq!(
                state.observe_activity(ts(1), Duration::from_secs(1)),
                PainStateUpdateEffect::Unchanged
            );
            assert_eq!(
                state.observe_activity(ts(30 * 60), Duration::from_secs(30 * 60 - 1),),
                PainStateUpdateEffect::Unchanged
            );
            assert!(!state.pain_input_needed());
        }

        #[test]
        fn confirmation_resets_sustained_activity_checkpoint() {
            let mut state = PainCheckState::new(ts(0), ts(0), Duration::ZERO);

            assert_eq!(
                state.observe_activity(ts(20 * 60), Duration::from_secs(20 * 60),),
                PainStateUpdateEffect::Unchanged
            );
            assert_eq!(state.confirm(ts(20 * 60)), PainStateUpdateEffect::Unchanged);
            assert_eq!(
                state.observe_activity(ts(40 * 60), Duration::from_secs(40 * 60),),
                PainStateUpdateEffect::Unchanged
            );
            assert!(!state.pain_input_needed());
        }

        #[test]
        fn reset_does_not_request_confirmation() {
            let mut state = PainCheckState::new(ts(0), ts(0), Duration::ZERO);

            assert_eq!(
                state.observe_credit_event(
                    ts(60 * 60),
                    CreditEvent::Reset {
                        kind: CreditKind::Rest,
                    },
                ),
                PainStateUpdateEffect::Unchanged
            );
            assert!(!state.pain_input_needed());
        }
    }
}
