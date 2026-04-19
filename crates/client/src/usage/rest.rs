use std::{pin::pin, time::Duration};

use bachelor::{
    broadcast::spmc::SpmcBroadcastConsumer,
    watch::{MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch},
};
use futures::future::{Either, select};
use shared::{
    model::UsageSnapshot,
    protocol::UsageIncrement,
    time::{boot_instant::BootInstant, timer::BoottimeTimer},
};

use crate::usage::StartupGap;

/// Inactivity duration that triggers a micro-rest reset.
pub const REST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct RestState {
    usage: UsageSnapshot,
}

pub fn create(
    usage_rx: SpmcBroadcastConsumer<UsageIncrement>,
    initial_state: RestState,
    startup_gap: StartupGap,
) -> (MpmcWatchRefSource<RestState>, impl Future) {
    let (state_tx, state_source) = mpmc_watch(initial_state);
    let driver = Driver { usage_rx, state_tx };
    (state_source, driver.run(startup_gap))
}

pub struct Driver {
    usage_rx: SpmcBroadcastConsumer<UsageIncrement>,
    state_tx: MpmcWatchRefProducer<RestState>,
}

impl Driver {
    async fn recv_activity(&mut self) {
        let Self { usage_rx, state_tx } = self;
        let _ = usage_rx
            .recv_ref(|increment| state_tx.update(|state| state.usage += &increment.delta))
            .await;
    }

    /// Detects inactivity and triggers rest resets.
    ///
    /// Uses `CLOCK_BOOTTIME` so that time spent in system suspend counts toward
    /// the rest period. This driver is fully self-contained and handles:
    /// - Runtime inactivity (no increments arriving)
    /// - Gaps from app startup (via startup_gap parameter)
    /// - System suspend (boottime timer fires immediately on resume if deadline passed)
    pub async fn run(mut self, startup_gap: StartupGap) {
        let mut timer = match BoottimeTimer::new() {
            Ok(t) => t,
            Err(e) => {
                log::error!("Failed to create boottime timer for rest_driver: {}", e);
                return;
            }
        };

        // Handle startup gap
        if startup_gap.duration() >= REST_TIMEOUT {
            log::info!(
                "Triggering rest reset on startup: {} seconds since last activity",
                startup_gap.duration().as_secs()
            );
            let _ = self.state_tx.set(RestState::default());
        }

        // Calculate initial deadline accounting for startup gap credit
        // If startup_gap was 20s and REST_TIMEOUT is 30s, we only need to wait 10s more
        let remaining = REST_TIMEOUT.saturating_sub(startup_gap.duration());
        let mut deadline = BootInstant::now() + remaining;

        loop {
            let timed_out = {
                let sleep = timer.sleep_until(deadline);
                let activity = self.recv_activity();

                match select(pin!(sleep), pin!(activity)).await {
                    Either::Left((result, _)) => Some(result),
                    Either::Right((_, _)) => None,
                }
            };

            match timed_out {
                Some(Ok(())) => {
                    log::debug!("rest period completed");
                    let _ = self.state_tx.set(RestState::default());
                }
                Some(Err(e)) => {
                    log::error!("rest_driver timer error: {}", e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                None => {
                    // Activity arrived, reset deadline
                    deadline = BootInstant::now() + REST_TIMEOUT;
                    continue;
                }
            }

            // After rest completed, wait for next activity before starting a new rest timer
            self.recv_activity().await;
            deadline = BootInstant::now() + REST_TIMEOUT;
        }
    }
}
