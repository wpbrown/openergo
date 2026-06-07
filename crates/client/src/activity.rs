use crate::usage::all::AllState;
use bachelor::error::Closed;
use bachelor::signal::mpmc_latched::{
    MpmcLatchedSignalConsumer, MpmcLatchedSignalProducer, MpmcLatchedSignalSource,
    signal as mpmc_latched_signal,
};
use bachelor::watch::{
    MpmcWatchRefConsumer, MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch as mpmc_watch_ref,
};
use futures::FutureExt;
use futures::future::{Either, select};
use rootcause::prelude::*;
use serde::{Deserialize, Serialize};
use serde_with::{DurationNanoSeconds, serde_as};
use std::time::Duration;
use tokio::time::{Instant, timeout};

const SETTLEMENT_DRAINS: u8 = 5;
const DRAIN_INTERVAL: Duration = Duration::from_millis(100);

#[serde_as]
#[derive(Default, Clone, Copy, Serialize, Deserialize)]
pub struct ActivityState {
    #[serde_as(as = "DurationNanoSeconds<u64>")]
    total: Duration,
}

impl ActivityState {
    #[allow(dead_code)]
    pub fn total(&self) -> Duration {
        self.total
    }
}

#[derive(Clone)]
pub struct ActivityProducer(MpmcLatchedSignalProducer);

impl ActivityProducer {
    pub fn notify(&self) {
        self.0.notify();
    }
}

#[derive(Clone)]
pub struct ActivitySource(MpmcLatchedSignalSource);

impl ActivitySource {
    pub fn subscribe_forward(&self) -> ActivityConsumer {
        ActivityConsumer(self.0.subscribe_forward())
    }
}

pub struct ActivityConsumer(MpmcLatchedSignalConsumer);

impl ActivityConsumer {
    pub fn observe(&mut self) -> impl Future<Output = ()> + Unpin {
        self.0.observe()
    }
}

pub fn signal() -> (ActivityProducer, ActivitySource) {
    let (producer, source) = mpmc_latched_signal();
    (ActivityProducer(producer), ActivitySource(source))
}

#[derive(Clone)]
pub struct ActivityStateSource(MpmcWatchRefSource<ActivityState>);

impl ActivityStateSource {
    pub fn subscribe_forward(&self) -> ActivityStateConsumer {
        ActivityStateConsumer(self.0.subscribe_forward())
    }
}

pub struct ActivityStateConsumer(MpmcWatchRefConsumer<ActivityState>);

impl ActivityStateConsumer {
    pub fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin {
        self.0.changed()
    }

    pub fn view<R>(&self, f: impl FnOnce(&ActivityState) -> R) -> R {
        self.0.view(f)
    }
}

impl crate::watch_mux::FiniteChanges for ActivityStateConsumer {
    fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin + '_ {
        ActivityStateConsumer::changed(self)
    }
}

pub fn create(
    initial: ActivityState,
    signal_rx: ActivityConsumer,
    usage_rx: MpmcWatchRefConsumer<AllState>,
) -> (
    ActivityStateSource,
    impl Future<Output = Result<(), Report>>,
) {
    let (state_tx, state_source) = mpmc_watch_ref(initial);
    let driver = Driver {
        signal_rx,
        usage_rx,
        state_tx,
    };
    (ActivityStateSource(state_source), driver.run())
}

struct Driver {
    signal_rx: ActivityConsumer,
    usage_rx: MpmcWatchRefConsumer<AllState>,
    state_tx: MpmcWatchRefProducer<ActivityState>,
}

impl Driver {
    async fn run(mut self) -> Result<(), Report> {
        let Self {
            signal_rx,
            usage_rx,
            state_tx,
        } = &mut self;

        loop {
            if !recv_event(signal_rx, usage_rx).await {
                return Ok(());
            }
            let mut start = Instant::now();
            let mut drains = 0;

            // Active loop.
            loop {
                let event = recv_event(signal_rx, usage_rx);
                let result = timeout(DRAIN_INTERVAL, event).await;
                let now = Instant::now();
                let gap = now.duration_since(start);
                let _ = state_tx.update(|state| state.total += gap);
                start = now;

                match result {
                    Ok(true) => drains = 0,
                    Ok(false) => return Ok(()),
                    Err(_) => {
                        drains += 1;
                        if drains >= SETTLEMENT_DRAINS {
                            break;
                        }
                    }
                }
            }
        }
    }
}

fn recv_event(
    activity_rx: &mut ActivityConsumer,
    usage_rx: &mut MpmcWatchRefConsumer<AllState>,
) -> impl Future<Output = bool> + Unpin {
    let signal = activity_rx.observe();
    let usage = usage_rx.changed();
    select(signal, usage).map(|either| match either {
        Either::Left(((), _)) | Either::Right((Ok(()), _)) => true,
        Either::Right((Err(Closed), _)) => false,
    })
}
