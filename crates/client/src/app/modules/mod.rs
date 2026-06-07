pub mod config;
pub mod credit;
pub mod endpoints;
pub mod ipc_server;
pub mod pain;
pub mod pain_integration;
pub mod persistence;
pub mod server_link;
pub mod telemetry;
pub mod transports;
pub mod usage;
pub mod utilization_integration;

pub mod fdr {
    use crate::activity::ActivityStateConsumer;
    use crate::credit::limit::CreditLimitConsumer;
    use crate::credit::utilization::{CreditEventConsumer, CreditUtilizationConsumer};
    use crate::fdr::{self, FdrConsumers};
    use crate::pain::PainConsumer;
    use crate::persistence::AppStateIdentity;
    use crate::usage::{AllUsageConsumer, UsageRawConsumer};
    use rootcause::prelude::*;
    use shared::oe_spawn;
    use shared::spawn::JoinHandle;

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        identity: AppStateIdentity,
        usage: AllUsageConsumer,
        usage_raw: UsageRawConsumer,
        pain: PainConsumer,
        limits: CreditLimitConsumer,
        activity: ActivityStateConsumer,
        events: CreditEventConsumer,
        events_utilization: CreditUtilizationConsumer,
    ) -> JoinHandle<Result<(), Report>> {
        let consumers = FdrConsumers {
            usage,
            usage_raw,
            pain,
            limits,
            activity,
            events,
            events_utilization,
        };
        oe_spawn!("flight-data-recorder", fdr::create(identity, consumers))
    }
}

pub mod notifications {
    use super::super::config;
    use crate::credit::utilization::CreditEventConsumer;
    use crate::notifications::{self, NotificationSettings};
    use futures::FutureExt;
    use rootcause::prelude::*;
    use shared::oe_spawn;
    use shared::spawn::JoinHandle;

    pub fn run(
        cfg: config::CreditNotificationsConfig,
        events: CreditEventConsumer,
    ) -> Option<JoinHandle<Result<(), Report>>> {
        let settings = NotificationSettings::new(cfg.notifications, cfg.sounds);
        if !settings.any() {
            return None;
        }
        Some(oe_spawn!(
            "notifications",
            notifications::create(settings, events).map(|()| Ok(()))
        ))
    }
}

pub mod activity {
    use crate::activity::{
        self, ActivityProducer, ActivitySource, ActivityState, ActivityStateSource,
    };
    use crate::usage::all::AllState;
    use bachelor::watch::MpmcWatchRefConsumer;
    use rootcause::prelude::*;
    use shared::oe_spawn;
    use shared::spawn::JoinHandle;

    pub struct ActivityModule {
        initial: ActivityState,
        signal_source: ActivitySource,
    }

    impl ActivityModule {
        pub fn signal_source(&self) -> &ActivitySource {
            &self.signal_source
        }

        pub fn start(self, usage_rx: MpmcWatchRefConsumer<AllState>) -> ActivityRuntime {
            let Self {
                initial,
                signal_source,
            } = self;
            let signal_rx = signal_source.subscribe_forward();
            let (state_source, driver) = activity::create(initial, signal_rx, usage_rx);
            let task = oe_spawn!("activity-driver", driver);
            ActivityRuntime { state_source, task }
        }
    }

    pub struct ActivityRuntime {
        state_source: ActivityStateSource,
        task: JoinHandle<Result<(), Report>>,
    }

    impl ActivityRuntime {
        pub fn state_source(&self) -> &ActivityStateSource {
            &self.state_source
        }

        pub fn detach(self) -> JoinHandle<Result<(), Report>> {
            self.task
        }
    }

    pub fn init(initial: ActivityState) -> (ActivityProducer, ActivityModule) {
        let (producer, signal_source) = activity::signal();
        (
            producer,
            ActivityModule {
                initial,
                signal_source,
            },
        )
    }
}
