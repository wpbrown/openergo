use crate::activity::ActivityConsumer;
use crate::credit::CreditIncrement;
use crate::usage::all::AllState;
use crate::usage::breaks::BreakState;
use crate::usage::daily::DayState;
use crate::usage::rest::RestState;
use crate::usage::{self, AllUsageSources, StartupGap, UsageRawProducer, UsageRawSource};
use bachelor::broadcast::spmc::broadcast;
use futures::FutureExt;
use rootcause::prelude::*;
use shared::oe_spawn;
use shared::protocol::server::UsageIncrement;
use shared::spawn::JoinHandle;
use std::num::NonZeroUsize;

const USAGE_BROADCAST_CAPACITY: NonZeroUsize =
    NonZeroUsize::new(16).expect("broadcast capacity must be non-zero");

/// Live handles produced by [`run`]. Use
/// [`Self::sources`] to subscribe (or clone the bundle) while the
/// runtime is live, then [`Self::detach`] at startup's tail to extract
/// the broadcast producer and the join handles for the final wiring
/// step.
pub struct UsageRuntime {
    producer: UsageRawProducer,
    raw_source: UsageRawSource,
    sources: AllUsageSources,
    tasks: Vec<JoinHandle<Result<(), Report>>>,
}

impl UsageRuntime {
    /// Cloneable bundle over the four watch sources. Callers either
    /// clone it (for handing to the IPC server) or call
    /// `subscribe_forward` on it.
    pub fn sources(&self) -> &AllUsageSources {
        &self.sources
    }

    /// Raw broadcast source of `(UsageIncrement, CreditIncrement)`
    /// pairs as forwarded from the upstream server, before any per-
    /// tracker aggregation. Use this when a consumer needs to observe
    /// every increment rather than the latched watch state.
    pub fn raw_source(&self) -> &UsageRawSource {
        &self.raw_source
    }

    /// Consume the runtime, returning the broadcast producer (for the
    /// upstream-server reconnect loop) and the four driver join
    /// handles (for shutdown orchestration). The internal
    /// [`AllUsageSources`] bundle is dropped here, but every
    /// outstanding clone or subscription created via [`Self::sources`]
    /// stays alive.
    pub fn detach(self) -> (UsageRawProducer, Vec<JoinHandle<Result<(), Report>>>) {
        let Self {
            producer,
            raw_source: _,
            sources: _,
            tasks,
        } = self;
        (producer, tasks)
    }
}

/// Construct the usage broadcast pair, spawn the four driver tasks,
/// and return everything the caller needs to wire up downstream
/// modules.
pub fn run(
    initial_all: AllState,
    initial_rest: RestState,
    initial_break: BreakState,
    initial_day: DayState,
    rest_activity_rx: Option<ActivityConsumer>,
) -> UsageRuntime {
    let startup_gap = StartupGap::since(initial_all.last_activity());

    let (producer, source) =
        broadcast::<(UsageIncrement, CreditIncrement)>(USAGE_BROADCAST_CAPACITY);

    let (rest_source, rest_driver) = usage::rest::create(
        source.subscribe(),
        rest_activity_rx,
        initial_rest,
        startup_gap,
    );
    let rest_task = oe_spawn!("rest-driver", rest_driver);

    let (break_source, break_driver) =
        usage::breaks::create(source.subscribe(), initial_break, startup_gap);
    let break_task = oe_spawn!("break-driver", break_driver);

    let (day_source, daily_driver) = usage::daily::create(source.subscribe(), initial_day);
    let daily_task = oe_spawn!("daily-driver", daily_driver);

    let (all_source, all_usage_driver) = usage::all::create(source.subscribe(), initial_all);
    let all_task = oe_spawn!("all-usage-driver", all_usage_driver.map(|()| Ok(())));

    UsageRuntime {
        producer,
        raw_source: source,
        sources: AllUsageSources::new(all_source, rest_source, break_source, day_source),
        tasks: vec![all_task, rest_task, break_task, daily_task],
    }
}
