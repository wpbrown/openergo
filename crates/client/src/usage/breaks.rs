use crate::credit::{CreditIncrement, SplitCreditSnapshot};
use crate::usage::StartupGap;
use bachelor::broadcast::SpmcBroadcastConsumer;
use bachelor::error::Closed;
use bachelor::watch::{MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch};
use futures::future::{Either, select};
use futures::pin_mut;
use rootcause::Report;
use rootcause::prelude::ResultExt;
use serde::{Deserialize, Serialize};
use shared::model::UsageSnapshot;
use shared::protocol::server::UsageIncrement;
use shared::time::boot_instant::BootInstant;
use shared::time::timer::BoottimeTimer;
use std::time::Duration;
use tracing::{debug, info, trace};

pub const BREAK_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Default, Serialize, Deserialize)]
pub struct BreakState {
    usage: UsageSnapshot,
    credit: SplitCreditSnapshot,
}

impl BreakState {
    #[cfg(test)]
    pub fn usage(&self) -> &UsageSnapshot {
        &self.usage
    }

    pub fn credit(&self) -> &SplitCreditSnapshot {
        &self.credit
    }

    fn apply_increment(&mut self, increment: &UsageIncrement, credit: &CreditIncrement) {
        self.usage += &increment.delta;
        self.credit += credit;
    }
}

pub fn create(
    usage_rx: SpmcBroadcastConsumer<(UsageIncrement, CreditIncrement)>,
    initial_state: BreakState,
    startup_gap: StartupGap,
) -> (
    MpmcWatchRefSource<BreakState>,
    impl Future<Output = Result<(), Report>>,
) {
    let (state_tx, state_source) = mpmc_watch(initial_state);
    let driver = Driver { usage_rx, state_tx };
    (state_source, driver.run(startup_gap))
}

pub struct Driver {
    usage_rx: SpmcBroadcastConsumer<(UsageIncrement, CreditIncrement)>,
    state_tx: MpmcWatchRefProducer<BreakState>,
}

enum DriverStep {
    NewActivity,
    TimeOut,
    Closed,
}

impl Driver {
    const CHECK_INTERVAL: Duration = Duration::from_secs(10);

    async fn recv_activity(&mut self) -> Result<(), Closed> {
        let Self { usage_rx, state_tx } = self;
        usage_rx
            .recv_ref(|(increment, credit)| {
                let _ = state_tx.update(|state| {
                    state.apply_increment(increment, credit);
                });
            })
            .await
    }

    async fn next_step(&mut self, timer: &mut BoottimeTimer) -> Result<DriverStep, Report> {
        let sleep = timer.sleep(Self::CHECK_INTERVAL);
        let activity = self.recv_activity();
        pin_mut!(sleep, activity);

        match select(sleep, activity).await {
            Either::Left((result, _)) => {
                result.context("step timer failed")?;
                Ok(DriverStep::TimeOut)
            }
            Either::Right((result, _)) => {
                if matches!(result, Err(Closed)) {
                    Ok(DriverStep::Closed)
                } else {
                    Ok(DriverStep::NewActivity)
                }
            }
        }
    }

    /// Detects inactivity and triggers break resets.
    ///
    /// Uses `CLOCK_BOOTTIME` so that time spent in system suspend counts toward
    /// the break period. This driver is fully self-contained and handles:
    /// - Runtime inactivity (no increments arriving)
    /// - Gaps from app startup (via startup_gap parameter)
    /// - System suspend (boottime timer fires immediately on resume if deadline passed)
    ///
    /// Unlike rest_driver, this allows partial progress: small bursts of activity
    /// don't fully reset the accumulated rest time.
    pub async fn run(mut self, startup_gap: StartupGap) -> Result<(), Report> {
        let mut timer = BoottimeTimer::new().context("failed to create boottime timer")?;

        // Initialize accumulated rest with startup gap (capped at threshold)
        let mut accumulated_rest = startup_gap.duration().min(BREAK_TIMEOUT);
        let mut last_check = BootInstant::now();
        let mut last_decay = BootInstant::now();

        // Check if break should trigger immediately
        if accumulated_rest >= BREAK_TIMEOUT {
            info!(
                startup_gap_secs = startup_gap.as_secs(),
                "triggering break reset on startup"
            );
            let _ = self.state_tx.set(BreakState::default());
            accumulated_rest = Duration::ZERO;

            // Wait for activity before starting to accumulate again
            self.recv_activity().await?;
            let now = BootInstant::now();
            last_check = now;
            last_decay = now;
        } else {
            debug!(
                startup_gap_secs = startup_gap.as_secs(),
                "startup gap below threshold, waiting for break period"
            );
        }

        loop {
            {
                let step = self.next_step(&mut timer).await?;

                match step {
                    DriverStep::TimeOut => {
                        let now = BootInstant::now();
                        let elapsed = now.saturating_duration_since(last_check);
                        last_check = now;

                        // All elapsed time counts as rest (no activity during this period)
                        accumulated_rest += elapsed;

                        trace!(
                            accumulated_rest_secs = accumulated_rest.as_secs(),
                            "new full rest period"
                        );
                    }
                    DriverStep::NewActivity => {
                        // Activity arrived
                        let now = BootInstant::now();

                        // Credit rest time since last_check (handles suspend case too)
                        // Only count if significant (> CHECK_INTERVAL)
                        let rest_since_check = now.saturating_duration_since(last_check);
                        if rest_since_check > Self::CHECK_INTERVAL {
                            trace!(
                                accumulated_rest_secs = accumulated_rest.as_secs(),
                                duration_secs = rest_since_check.as_secs(),
                                "new rest before activity"
                            );
                            accumulated_rest += rest_since_check;
                        }

                        last_check = now;

                        // Decay accumulated rest, but only once per CHECK_INTERVAL
                        // This prevents rapid activity from decaying multiple times
                        let time_since_decay = now.saturating_duration_since(last_decay);
                        if time_since_decay >= Self::CHECK_INTERVAL {
                            accumulated_rest =
                                accumulated_rest.saturating_sub(Self::CHECK_INTERVAL);
                            last_decay = now;
                            trace!(
                                accumulated_rest_secs = accumulated_rest.as_secs(),
                                "decayed accumulated rest due to activity"
                            );
                        }
                    }
                    DriverStep::Closed => return Ok(()),
                }
            };

            let break_completed = if accumulated_rest >= BREAK_TIMEOUT {
                debug!(
                    accumulated_rest_secs = accumulated_rest.as_secs(),
                    "break period completed"
                );
                true
            } else {
                false
            };

            // After break completed, wait for activity before starting to accumulate again
            if break_completed {
                let _ = self.state_tx.set(BreakState::default());
                accumulated_rest = Duration::ZERO;
                self.recv_activity().await?;
                let now = BootInstant::now();
                last_check = now;
                last_decay = now;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credit::{Credit, CreditDelta, HandCreditDelta};
    use jiff::Timestamp;
    use shared::model::{HandUsageDelta, ModifierUsageDelta, UsageDelta};

    fn increment(delta: UsageDelta) -> UsageIncrement {
        UsageIncrement::new(
            delta,
            Timestamp::from_second(1).unwrap(),
            Timestamp::from_second(2).unwrap(),
        )
    }

    #[test]
    fn accumulates_handed_usage_and_compact_credit() {
        let mut state = BreakState::default();
        let increment = increment(UsageDelta {
            right: HandUsageDelta {
                modifier: ModifierUsageDelta {
                    same_hand_combo: 3,
                    ..ModifierUsageDelta::default()
                },
                ..HandUsageDelta::default()
            },
            ..UsageDelta::default()
        });
        let credit = CreditIncrement {
            base: CreditDelta {
                right: HandCreditDelta {
                    modifier: Credit::new(4.0),
                    ..HandCreditDelta::default()
                },
                ..CreditDelta::default()
            },
            boost: CreditDelta::default(),
        };

        state.apply_increment(&increment, &credit);

        assert_eq!(state.usage().right.modifier.same_hand_combo, 3);
        assert_eq!(state.credit().base.right.modifier, Credit::new(4.0));
    }
}
