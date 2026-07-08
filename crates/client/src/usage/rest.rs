use crate::activity::ActivityConsumer;
use crate::credit::{CreditIncrement, SplitCreditSnapshot};
use crate::usage::StartupGap;
use bachelor::broadcast::spmc::SpmcBroadcastConsumer;
use bachelor::error::Closed;
use bachelor::watch::{MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch};
use futures::future::{Either, select};
use rootcause::prelude::*;
use serde::{Deserialize, Serialize};
use shared::model::UsageSnapshot;
use shared::protocol::server::UsageIncrement;
use shared::time::boot_instant::BootInstant;
use shared::time::timer::BoottimeTimer;
use std::pin::pin;
use std::time::Duration;
use tracing::{debug, info};

/// Inactivity duration that triggers a micro-rest reset.
pub const REST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default, Serialize, Deserialize)]
pub struct RestState {
    usage: UsageSnapshot,
    credit: SplitCreditSnapshot,
}

impl RestState {
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
    activity_rx: Option<ActivityConsumer>,
    initial_state: RestState,
    startup_gap: StartupGap,
) -> (
    MpmcWatchRefSource<RestState>,
    impl Future<Output = Result<(), Report>>,
) {
    let (state_tx, state_source) = mpmc_watch(initial_state);
    let driver = Driver {
        usage_rx,
        activity_rx,
        state_tx,
    };
    (state_source, driver.run(startup_gap))
}

pub struct Driver {
    usage_rx: SpmcBroadcastConsumer<(UsageIncrement, CreditIncrement)>,
    activity_rx: Option<ActivityConsumer>,
    state_tx: MpmcWatchRefProducer<RestState>,
}

enum DriverStep {
    NewUsage,
    TimeOut,
    Closed,
}

impl Driver {
    async fn recv_usage_impl(
        usage_rx: &mut SpmcBroadcastConsumer<(UsageIncrement, CreditIncrement)>,
        state_tx: &mut MpmcWatchRefProducer<RestState>,
    ) -> Result<(), Closed> {
        usage_rx
            .recv_ref(|(increment, credit)| {
                let _ = state_tx.update(|state| {
                    state.apply_increment(increment, credit);
                });
            })
            .await
    }

    async fn recv_usage(&mut self) -> Result<(), Closed> {
        Self::recv_usage_impl(&mut self.usage_rx, &mut self.state_tx).await
    }

    async fn next_step(
        &mut self,
        timer: &mut BoottimeTimer,
        deadline: BootInstant,
    ) -> Result<DriverStep, Report> {
        let Self {
            usage_rx,
            activity_rx,
            state_tx,
        } = self;
        let sleep = pin!(timer.sleep_until(deadline));
        let usage = pin!(Self::recv_usage_impl(usage_rx, state_tx));
        let activity = match activity_rx.as_mut() {
            Some(rx) => Either::Left(rx.observe()),
            None => Either::Right(std::future::pending()),
        };

        match select(sleep, select(usage, activity)).await {
            Either::Left((result, _)) => {
                result.context("rest driver timer error")?;
                Ok(DriverStep::TimeOut)
            }
            Either::Right((Either::Left((Err(Closed), _)), _)) => Ok(DriverStep::Closed),
            Either::Right((Either::Left((Ok(()), _)), _))
            | Either::Right((Either::Right(((), _)), _)) => Ok(DriverStep::NewUsage),
        }
    }

    /// Detects inactivity and triggers rest resets.
    ///
    /// Uses `CLOCK_BOOTTIME` so that time spent in system suspend counts toward
    /// the rest period. This driver is fully self-contained and handles:
    /// - Runtime inactivity (no user input, usage or activity ping, arriving)
    /// - Gaps from app startup (via startup_gap parameter)
    /// - System suspend (boottime timer fires immediately on resume if deadline passed)
    pub async fn run(mut self, startup_gap: StartupGap) -> Result<(), Report> {
        let mut timer =
            BoottimeTimer::new().context("Failed to create boottime timer for rest driver")?;

        // Handle startup gap
        if startup_gap.duration() >= REST_TIMEOUT {
            info!(
                startup_gap_secs = startup_gap.duration().as_secs(),
                "triggering rest reset on startup"
            );
            let _ = self.state_tx.set(RestState::default());
        } else {
            debug!(
                startup_gap_secs = startup_gap.duration().as_secs(),
                "startup gap below threshold, waiting for rest timeout"
            );
        }

        // Calculate initial deadline accounting for startup gap credit
        // If startup_gap was 20s and REST_TIMEOUT is 30s, we only need to wait 10s more
        let remaining = REST_TIMEOUT.saturating_sub(startup_gap.duration());
        let mut deadline = BootInstant::now() + remaining;

        loop {
            match self.next_step(&mut timer, deadline).await? {
                DriverStep::TimeOut => {
                    debug!("rest period completed");
                    let _ = self.state_tx.set(RestState::default());
                }
                DriverStep::NewUsage => {
                    // Input arrived, reset deadline
                    deadline = BootInstant::now() + REST_TIMEOUT;
                    continue;
                }
                DriverStep::Closed => return Ok(()),
            }

            // After rest completed, wait for next input before starting a new rest timer
            if matches!(self.recv_usage().await, Err(Closed)) {
                return Ok(());
            }
            deadline = BootInstant::now() + REST_TIMEOUT;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credit::{Credit, CreditDelta, HandCreditDelta};
    use jiff::Timestamp;
    use shared::model::{HandUsageDelta, UsageDelta};

    fn increment(delta: UsageDelta) -> UsageIncrement {
        UsageIncrement::new(
            delta,
            Timestamp::from_second(1).unwrap(),
            Timestamp::from_second(2).unwrap(),
        )
    }

    #[test]
    fn accumulates_handed_usage_and_compact_credit() {
        let mut state = RestState::default();
        let increment = increment(UsageDelta {
            left: HandUsageDelta {
                scroll_count: 4,
                ..HandUsageDelta::default()
            },
            unclassified_key_combo: 2,
            ..UsageDelta::default()
        });
        let credit = CreditIncrement {
            base: CreditDelta {
                left: HandCreditDelta {
                    scroll: Credit::new(1.0),
                    ..HandCreditDelta::default()
                },
                unclassified_key: Credit::new(2.0),
                ..CreditDelta::default()
            },
            boost: CreditDelta::default(),
        };

        state.apply_increment(&increment, &credit);

        assert_eq!(state.usage().left.scroll_count, 4);
        assert_eq!(state.usage().unclassified_key_combo, 2);
        assert_eq!(state.credit().base.left.scroll, Credit::new(1.0));
        assert_eq!(state.credit().base.unclassified_key, Credit::new(2.0));
    }
}
