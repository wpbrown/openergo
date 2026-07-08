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

    fn apply_increment(&mut self, increment: &UsageIncrement, credit: &CreditIncrement) {
        self.usage += &increment.delta;
        self.credit += credit;
        self.last_activity = increment.end;
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
                    state.apply_increment(increment, credit);
                });
            })
            .await
    }

    async fn run(mut self) {
        while self.recv_activity().await.is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credit::{Credit, CreditDelta, HandCreditDelta};
    use jiff::Timestamp;
    use shared::model::{HandUsageDelta, UsageDelta};
    use std::time::Duration;

    fn increment(delta: UsageDelta) -> UsageIncrement {
        UsageIncrement::new(
            delta,
            Timestamp::from_second(1).unwrap(),
            Timestamp::from_second(2).unwrap(),
        )
    }

    fn credit() -> CreditIncrement {
        CreditIncrement {
            base: CreditDelta {
                left: HandCreditDelta {
                    key: Credit::new(2.0),
                    ..HandCreditDelta::default()
                },
                unclassified_key: Credit::new(1.0),
                ..CreditDelta::default()
            },
            boost: CreditDelta {
                right: HandCreditDelta {
                    modifier: Credit::new(3.0),
                    ..HandCreditDelta::default()
                },
                ..CreditDelta::default()
            },
        }
    }

    #[test]
    fn accumulates_handed_usage_and_compact_credit() {
        let mut state = AllState::default();
        let increment = increment(UsageDelta {
            left: HandUsageDelta {
                key_count: 2,
                ..HandUsageDelta::default()
            },
            right: HandUsageDelta {
                click_count: 1,
                ..HandUsageDelta::default()
            },
            unclassified_key_count: 3,
            active_duration: Duration::from_secs(1),
            ..UsageDelta::default()
        });

        state.apply_increment(&increment, &credit());

        assert_eq!(state.usage.left.key_count, 2);
        assert_eq!(state.usage.right.click_count, 1);
        assert_eq!(state.usage.unclassified_key_count, 3);
        assert_eq!(state.credit.base.left.key, Credit::new(2.0));
        assert_eq!(state.credit.base.unclassified_key, Credit::new(1.0));
        assert_eq!(state.credit.boost.right.modifier, Credit::new(3.0));
        assert_eq!(state.last_activity, increment.end);
    }
}
