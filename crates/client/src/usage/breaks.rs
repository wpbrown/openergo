use std::{pin::pin, time::Duration};

use bachelor::{
    broadcast::SpmcBroadcastConsumer,
    error::Closed,
    watch::{MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch},
};
use futures::future::{Either, select};
use rootcause::{Report, prelude::ResultExt};
use shared::{
    model::UsageSnapshot,
    protocol::UsageIncrement,
    time::{boot_instant::BootInstant, timer::BoottimeTimer},
};

use crate::usage::StartupGap;

pub const BREAK_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Default)]
pub struct BreakState {
    usage: UsageSnapshot,
}

pub fn create(
    usage_rx: SpmcBroadcastConsumer<UsageIncrement>,
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
    usage_rx: SpmcBroadcastConsumer<UsageIncrement>,
    state_tx: MpmcWatchRefProducer<BreakState>,
}

impl Driver {
    async fn recv_activity(&mut self) -> Result<(), Closed> {
        let Self { usage_rx, state_tx } = self;
        usage_rx
            .recv_ref(|increment| {
                let _ = state_tx.update(|state| state.usage += &increment.delta);
            })
            .await
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
        let mut timer =
            BoottimeTimer::new().context("Failed to create boottime timer for break driver")?;

        // Initialize accumulated rest with startup gap (capped at threshold)
        let mut accumulated_rest = startup_gap.duration().min(BREAK_TIMEOUT);
        let mut last_check = BootInstant::now();
        let mut last_decay = BootInstant::now();

        // Check if break should trigger immediately
        if accumulated_rest >= BREAK_TIMEOUT {
            log::info!(
                "Triggering break reset on startup: {} seconds since last activity",
                startup_gap.as_secs()
            );
            let _ = self.state_tx.set(BreakState::default());
            accumulated_rest = Duration::ZERO;

            // Wait for activity before starting to accumulate again
            self.recv_activity().await?;
            let now = BootInstant::now();
            last_check = now;
            last_decay = now;
        }

        const CHECK_INTERVAL: Duration = Duration::from_secs(10);

        loop {
            let break_completed = {
                let sleep = timer.sleep(CHECK_INTERVAL);
                let activity = self.recv_activity();

                match select(pin!(sleep), pin!(activity)).await {
                    Either::Left((result, _)) => {
                        if let Err(e) = result {
                            log::error!("break_driver timer error: {}", e);
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }

                        // Timer fired — accumulate rest time since last check
                        let now = BootInstant::now();
                        let elapsed = now.saturating_duration_since(last_check);
                        last_check = now;

                        // All elapsed time counts as rest (no activity during this period)
                        accumulated_rest += elapsed;

                        if accumulated_rest >= BREAK_TIMEOUT {
                            log::debug!("break period completed");
                            true
                        } else {
                            false
                        }
                    }
                    Either::Right((result, _)) => {
                        if result.is_err() {
                            return Ok(());
                        }

                        // Activity arrived
                        let now = BootInstant::now();

                        // Credit rest time since last_check (handles suspend case too)
                        // Only count if significant (> CHECK_INTERVAL)
                        let rest_since_check = now.saturating_duration_since(last_check);
                        if rest_since_check > CHECK_INTERVAL {
                            accumulated_rest += rest_since_check;
                        }

                        last_check = now;

                        // Decay accumulated rest, but only once per CHECK_INTERVAL
                        // This prevents rapid activity from decaying multiple times
                        let time_since_decay = now.saturating_duration_since(last_decay);
                        if time_since_decay >= CHECK_INTERVAL {
                            accumulated_rest = accumulated_rest.saturating_sub(CHECK_INTERVAL);
                            last_decay = now;
                        }

                        if accumulated_rest >= BREAK_TIMEOUT {
                            log::debug!("break period completed (with rest time)");
                            true
                        } else {
                            false
                        }
                    }
                }
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
