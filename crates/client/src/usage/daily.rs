use crate::credit::{CreditIncrement, SplitCreditSnapshot};
use bachelor::broadcast::spmc::SpmcBroadcastConsumer;
use bachelor::watch::{MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch};
use futures::future::{Either, select};
use jiff::civil::Time;
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use rootcause::prelude::*;
use serde::{Deserialize, Serialize};
use shared::model::UsageSnapshot;
use shared::oe_spawn;
use shared::protocol::server::UsageIncrement;
use shared::spawn::JoinHandle;
use shared::time::timer::{RealtimeSleepEnd, RealtimeTimer};
use std::pin::pin;
use tracing::{debug, info, warn};

/// Time of day at which the daily usage counters reset.
const RESET_TIME: Time = Time::constant(3, 0, 0, 0); // 3:00:00 AM

#[derive(Serialize, Deserialize)]
pub struct DayState {
    usage: UsageSnapshot,
    credit: SplitCreditSnapshot,
    last_reset: Timestamp,
}

impl Default for DayState {
    fn default() -> Self {
        Self {
            usage: UsageSnapshot::default(),
            credit: SplitCreditSnapshot::default(),
            last_reset: Timestamp::now(),
        }
    }
}

impl DayState {
    pub fn credit(&self) -> &SplitCreditSnapshot {
        &self.credit
    }
}

pub fn create(
    usage_rx: SpmcBroadcastConsumer<(UsageIncrement, CreditIncrement)>,
    initial_state: DayState,
) -> (
    MpmcWatchRefSource<DayState>,
    impl Future<Output = Result<(), Report>>,
) {
    let (state_tx, state_source) = mpmc_watch(initial_state);

    // Accumulate activity into the day state on a separate task so the timer
    // loop only deals with scheduling resets. The run loop watches this
    // handle so it shuts down once the broadcast closes.
    let accumulate_task = oe_spawn!("daily-accumulate", accumulate(usage_rx, state_tx.clone()));

    (state_source, run(state_tx, accumulate_task))
}

async fn accumulate(
    mut usage_rx: SpmcBroadcastConsumer<(UsageIncrement, CreditIncrement)>,
    state_tx: MpmcWatchRefProducer<DayState>,
) {
    while usage_rx
        .recv_ref(|(increment, credit)| {
            let _ = state_tx.update(|state| {
                state.usage += &increment.delta;
                state.credit += credit;
            });
        })
        .await
        .is_ok()
    {}
}

/// Resets the daily usage counters at a configured wall-clock time.
///
/// Uses `CLOCK_REALTIME` so that the reset is anchored to the user's
/// local clock. Handles system clock changes (e.g., NTP, DST) by
/// recomputing the next target whenever the timer is cancelled.
///
/// Exits when `accumulate_task` completes (which happens when the usage
/// broadcast closes during shutdown).
async fn run(
    state_tx: MpmcWatchRefProducer<DayState>,
    mut accumulate_task: JoinHandle<()>,
) -> Result<(), Report> {
    let mut timer =
        RealtimeTimer::new().context("Failed to create realtime timer for daily driver")?;

    // Initialize from persisted state
    let last_reset = state_tx.view(|s| s.last_reset);
    let last_reset_zoned = last_reset.to_zoned(TimeZone::system());
    let mut last_reset_date = Some(last_reset_zoned.date());

    loop {
        let now = Zoned::now();
        let today = now.date();

        // Check if we should perform a reset right now.
        // This handles the case where we wake up after the reset time but
        // haven't reset today yet.
        if now.time() >= RESET_TIME && last_reset_date != Some(today) {
            info!("reset triggered at {}", now);
            let _ = state_tx.set(DayState {
                usage: UsageSnapshot::default(),
                credit: SplitCreditSnapshot::default(),
                last_reset: now.timestamp(),
            });
            last_reset_date = Some(today);
        }

        let target = calculate_next_reset_time(&now, &RESET_TIME);

        debug!(
            "sleeping until {} ({} from now)",
            target,
            now.duration_until(&target),
        );

        let sleep = timer.sleep_until(target.timestamp());
        match select(pin!(sleep), &mut accumulate_task).await {
            Either::Left((result, _)) => match result.context("Daily driver timer error")? {
                RealtimeSleepEnd::Completed => debug!("sleep until {} completed", target),
                RealtimeSleepEnd::Cancelled => {
                    warn!("timer cancelled due to system clock change");
                }
            },
            Either::Right(((), _)) => {
                debug!("accumulate task exited, shutting down daily driver");
                return Ok(());
            }
        }
    }
}

fn calculate_next_reset_time(now: &Zoned, reset_time: &Time) -> Zoned {
    fn expect_no_oob<T>(result: Result<T, jiff::Error>) -> T {
        result.expect("calculation should not be near bounds of Timestamp since we start from now")
    }

    let today = now.date();
    let datetime = today.at(
        reset_time.hour(),
        reset_time.minute(),
        reset_time.second(),
        reset_time.subsec_nanosecond(),
    );
    let target = expect_no_oob(now.time_zone().to_ambiguous_zoned(datetime).later());

    if target.timestamp() > now.timestamp() {
        target
    } else {
        let tomorrow_datetime = expect_no_oob(datetime.tomorrow());

        expect_no_oob(
            now.time_zone()
                .to_ambiguous_zoned(tomorrow_datetime)
                .later(),
        )
    }
}
