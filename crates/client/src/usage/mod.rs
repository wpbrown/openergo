use jiff::Timestamp;
use std::time::Duration;

pub mod all;
pub mod breaks;
pub mod daily;
pub mod rest;

use crate::credit::CreditIncrement;
pub use all_sources::{AllUsageConsumer, AllUsageSources};
use bachelor::broadcast::spmc::{
    SpmcBroadcastConsumer, SpmcBroadcastProducer, SpmcBroadcastSource,
};
use shared::protocol::server::UsageIncrement;

/// Producer half of the raw usage broadcast. The upstream-server
/// reconnect loop is the sole writer.
pub type UsageRawProducer = SpmcBroadcastProducer<(UsageIncrement, CreditIncrement)>;

/// Source half of the raw usage broadcast carrying every
/// `(UsageIncrement, CreditIncrement)` pair forwarded from the
/// upstream server.
pub type UsageRawSource = SpmcBroadcastSource<(UsageIncrement, CreditIncrement)>;

/// Consumer half of the raw usage broadcast. Obtained by calling
/// `subscribe()` on a [`UsageRawSource`].
pub type UsageRawConsumer = SpmcBroadcastConsumer<(UsageIncrement, CreditIncrement)>;

#[derive(Default, Clone, Copy)]
pub struct StartupGap(Duration);

impl StartupGap {
    /// Gap between `last_activity` and now.
    pub fn since(last_activity: Timestamp) -> Self {
        Self(
            Timestamp::now()
                .duration_since(last_activity)
                .unsigned_abs(),
        )
    }

    pub fn duration(&self) -> Duration {
        self.0
    }

    pub fn as_secs(&self) -> u64 {
        self.0.as_secs()
    }
}

mod all_sources {
    use super::all::AllState;
    use super::breaks::BreakState;
    use super::daily::DayState;
    use super::rest::RestState;
    use bachelor::error::Closed;
    use bachelor::watch::{MpmcWatchRefConsumer, MpmcWatchRefSource};
    use futures::FutureExt;
    use futures::future::{Either, select};
    use std::future::Future;

    /// Identifies which usage state source produced a change observation.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum UsageSource {
        All,
        Rest,
        Break,
        Day,
    }

    /// A bundle of consumer subscriptions to all four usage state sources,
    /// providing a unified `changed` and `view` API over them.
    pub struct AllUsageConsumer {
        all: MpmcWatchRefConsumer<AllState>,
        rest: MpmcWatchRefConsumer<RestState>,
        breaks: MpmcWatchRefConsumer<BreakState>,
        day: MpmcWatchRefConsumer<DayState>,
    }

    impl AllUsageConsumer {
        /// Subscribe forward to all four sources.
        pub fn subscribe_forward(
            all: &MpmcWatchRefSource<AllState>,
            rest: &MpmcWatchRefSource<RestState>,
            breaks: &MpmcWatchRefSource<BreakState>,
            day: &MpmcWatchRefSource<DayState>,
        ) -> Self {
            Self {
                all: all.subscribe_forward(),
                rest: rest.subscribe_forward(),
                breaks: breaks.subscribe_forward(),
                day: day.subscribe_forward(),
            }
        }

        /// Wait for any of the four sources to change. Returns which source
        /// produced the observation, or `Closed` if the observed source has
        /// closed.
        pub fn changed(&mut self) -> impl Future<Output = Result<UsageSource, Closed>> + Unpin {
            let all = self.all.changed();
            let rest = self.rest.changed();
            let brk = self.breaks.changed();
            let day = self.day.changed();
            select(select(all, rest), select(brk, day)).map(|watches| match watches {
                Either::Left((Either::Left((r, _)), _)) => r.map(|()| UsageSource::All),
                Either::Left((Either::Right((r, _)), _)) => r.map(|()| UsageSource::Rest),
                Either::Right((Either::Left((r, _)), _)) => r.map(|()| UsageSource::Break),
                Either::Right((Either::Right((r, _)), _)) => r.map(|()| UsageSource::Day),
            })
        }

        /// Borrow all four states simultaneously through nested `view` calls.
        pub fn view<R>(
            &self,
            f: impl FnOnce(&AllState, &RestState, &BreakState, &DayState) -> R,
        ) -> R {
            self.all.view(|all| {
                self.day.view(|day| {
                    self.rest
                        .view(|rest| self.breaks.view(|breaks| f(all, rest, breaks, day)))
                })
            })
        }
    }

    impl crate::watch_mux::FiniteChanges for AllUsageConsumer {
        fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin + '_ {
            AllUsageConsumer::changed(self).map(|res| res.map(|_| ()))
        }
    }

    /// Owned bundle of the four usage state sources. Each call to
    /// [`Self::subscribe_forward`] hands out a fresh [`AllUsageConsumer`]
    /// consumer subscribed to all four.
    #[derive(Clone)]
    pub struct AllUsageSources {
        all: MpmcWatchRefSource<AllState>,
        rest: MpmcWatchRefSource<RestState>,
        breaks: MpmcWatchRefSource<BreakState>,
        day: MpmcWatchRefSource<DayState>,
    }

    impl AllUsageSources {
        pub fn new(
            all: MpmcWatchRefSource<AllState>,
            rest: MpmcWatchRefSource<RestState>,
            breaks: MpmcWatchRefSource<BreakState>,
            day: MpmcWatchRefSource<DayState>,
        ) -> Self {
            Self {
                all,
                rest,
                breaks,
                day,
            }
        }

        pub fn subscribe_forward(&self) -> AllUsageConsumer {
            AllUsageConsumer::subscribe_forward(&self.all, &self.rest, &self.breaks, &self.day)
        }

        pub fn all(&self) -> &MpmcWatchRefSource<AllState> {
            &self.all
        }
    }
}
