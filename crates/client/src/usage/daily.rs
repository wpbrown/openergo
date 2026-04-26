use bachelor::{
    broadcast::spmc::SpmcBroadcastConsumer,
    watch::{MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch},
};
use jiff::{Timestamp, Zoned, civil::Time, tz::TimeZone};
use rootcause::prelude::*;
use shared::{
    model::UsageSnapshot,
    protocol::UsageIncrement,
    spawn::oe_spawn,
    time::timer::{RealtimeSleepEnd, RealtimeTimer},
};

/// Time of day at which the daily usage counters reset.
const RESET_TIME: Time = Time::constant(3, 0, 0, 0); // 3:00:00 AM

pub struct DayState {
    pub usage: UsageSnapshot,
    pub last_reset: Timestamp,
}

impl Default for DayState {
    fn default() -> Self {
        Self {
            usage: UsageSnapshot::default(),
            last_reset: Timestamp::now(),
        }
    }
}

pub fn create(
    usage_rx: SpmcBroadcastConsumer<UsageIncrement>,
    initial_state: DayState,
) -> (
    MpmcWatchRefSource<DayState>,
    impl Future<Output = Result<(), Report>>,
) {
    let (state_tx, state_source) = mpmc_watch(initial_state);

    // Accumulate activity into the day state on a separate task so the timer
    // loop only deals with scheduling resets.
    oe_spawn(accumulate(usage_rx, state_tx.clone()));

    (state_source, run(state_tx))
}

async fn accumulate(
    mut usage_rx: SpmcBroadcastConsumer<UsageIncrement>,
    state_tx: MpmcWatchRefProducer<DayState>,
) {
    while usage_rx
        .recv_ref(|increment| {
            let _ = state_tx.update(|state| state.usage += &increment.delta);
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
async fn run(state_tx: MpmcWatchRefProducer<DayState>) -> Result<(), Report> {
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
            log::info!("reset triggered at {}", now);
            let _ = state_tx.set(DayState {
                usage: UsageSnapshot::default(),
                last_reset: now.timestamp(),
            });
            last_reset_date = Some(today);
        }

        let target = calculate_next_reset_time(&now, &RESET_TIME);

        log::debug!(
            "sleeping until {} ({} from now)",
            target,
            now.duration_until(&target),
        );

        match timer
            .sleep_until(target.timestamp())
            .await
            .context("Daily driver timer error")?
        {
            RealtimeSleepEnd::Completed => log::debug!("sleep until {} completed", target),
            RealtimeSleepEnd::Cancelled => {
                log::warn!("timer cancelled due to system clock change");
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
