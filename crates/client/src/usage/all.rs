use crate::credit::{CreditIncrement, SplitCreditSnapshot};
use bachelor::broadcast::spmc::SpmcBroadcastConsumer;
use bachelor::error::Closed;
use bachelor::watch::{MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use shared::model::UsageSnapshot;
use shared::protocol::server::UsageIncrement;

/// Aggregated lifetime usage plus the end timestamp of the last
/// processed increment. The timestamp is used by the persistence layer
/// to compute the startup gap when the client restarts.
#[derive(Serialize, Deserialize)]
pub struct AllState {
    usage: UsageSnapshot,
    credit: SplitCreditSnapshot,
    last_activity: Timestamp,
}

impl Default for AllState {
    fn default() -> Self {
        Self {
            usage: UsageSnapshot::default(),
            credit: SplitCreditSnapshot::default(),
            last_activity: Timestamp::now(),
        }
    }
}

impl AllState {
    pub fn usage(&self) -> &UsageSnapshot {
        &self.usage
    }

    pub fn credit(&self) -> &SplitCreditSnapshot {
        &self.credit
    }

    pub fn last_activity(&self) -> Timestamp {
        self.last_activity
    }
}

pub fn create(
    usage_rx: SpmcBroadcastConsumer<(UsageIncrement, CreditIncrement)>,
    initial_state: AllState,
) -> (MpmcWatchRefSource<AllState>, impl Future<Output = ()>) {
    let (state_tx, state_source) = mpmc_watch(initial_state);
    let driver = Driver { usage_rx, state_tx };
    (state_source, driver.run())
}

struct Driver {
    usage_rx: SpmcBroadcastConsumer<(UsageIncrement, CreditIncrement)>,
    state_tx: MpmcWatchRefProducer<AllState>,
}

impl Driver {
    async fn recv_activity(&mut self) -> Result<(), Closed> {
        let Self { usage_rx, state_tx } = self;
        usage_rx
            .recv_ref(|(increment, credit)| {
                let _ = state_tx.update(|state| {
                    state.usage += &increment.delta;
                    state.credit += credit;
                    state.last_activity = increment.end;
                });
            })
            .await
    }

    async fn run(mut self) {
        while self.recv_activity().await.is_ok() {}
    }
}
