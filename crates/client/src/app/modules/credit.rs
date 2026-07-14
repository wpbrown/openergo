use crate::credit::CreditCalculator;
use crate::credit::calculator::config::CreditCalculatorConfig;
use crate::credit::limit::{self, CreditLimitProducer, CreditLimitSource, CreditLimitState};
use crate::credit::utilization::{
    self, CreditEventSource, CreditUtilizationSource, CreditUtilizationState,
};
use crate::integration::{AnalogOutProducer, Binder, EndpointConfig, EndpointLabel};
use crate::usage::AllUsageConsumer;
use futures::FutureExt;
use rootcause::prelude::*;
use shared::model::CreditLimit;
use shared::oe_spawn;
use shared::shutdown::ShutdownSource;
use shared::spawn::JoinHandle;

pub struct Config {
    pub limits: LimitsConfig,
    pub utilization: Option<UtilizationConfig>,
    pub calculator: CreditCalculatorConfig,
}

pub struct LimitsConfig {
    pub rest: f64,
    pub breaks: f64,
    pub day: f64,
}

pub struct UtilizationConfig {
    pub rest_sink: Option<String>,
    pub breaks_sink: Option<String>,
    pub day_sink: Option<String>,
}

/// The three optional [`AnalogOutProducer`] sinks bound from
/// `[credit.utilization]`. Handed to the
/// [`super::utilization_sink_forwarder`] connector by `app::run`.
pub struct CreditSinks {
    pub rest: Option<AnalogOutProducer>,
    pub breaks: Option<AnalogOutProducer>,
    pub day: Option<AnalogOutProducer>,
}

impl CreditSinks {
    /// `true` if at least one sink is bound, so callers can skip
    /// spawning the forwarder when there's nothing to push to.
    pub fn any(&self) -> bool {
        self.rest.is_some() || self.breaks.is_some() || self.day.is_some()
    }
}

pub struct CreditModule {
    utilization_cfg: Option<UtilizationConfig>,
    // Kept alive for the lifetime of the process so the limit watch
    // never closes. Reserved for the future pain-driven tuner.
    limit_producer: CreditLimitProducer,
    limit_source: CreditLimitSource,
}

impl CreditModule {
    /// Bind the optional rest/breaks/day `AnalogOut`s seeded with the
    /// persisted `last_published` ratio so transports that read the
    /// watch immediately at startup see the correct value.
    pub fn bind_sinks<T: EndpointConfig>(
        &self,
        binder: &mut Binder<T>,
        initial: &CreditUtilizationState,
    ) -> Result<CreditSinks, Report> {
        let Some(cfg) = self.utilization_cfg.as_ref() else {
            return Ok(CreditSinks {
                rest: None,
                breaks: None,
                day: None,
            });
        };
        Ok(CreditSinks {
            rest: bind_sink(
                binder,
                "rest_sink",
                cfg.rest_sink.as_deref(),
                initial.last_published().rest,
            )?,
            breaks: bind_sink(
                binder,
                "breaks_sink",
                cfg.breaks_sink.as_deref(),
                initial.last_published().breaks,
            )?,
            day: bind_sink(
                binder,
                "day_sink",
                cfg.day_sink.as_deref(),
                initial.last_published().day,
            )?,
        })
    }

    /// Spawn the utilization driver plus a keepalive task that owns
    /// the limit producer for the lifetime of the process. The
    /// keepalive task waits on `shutdown` and then exits, dropping
    /// the producer (the watch closes naturally when the last
    /// producer drops). Replace the keepalive with the future pain-
    /// driven tuner when it lands.
    pub fn start(
        self,
        usage: AllUsageConsumer,
        initial_util: CreditUtilizationState,
        shutdown: &ShutdownSource,
    ) -> CreditRuntime {
        let Self {
            limit_source,
            limit_producer,
            ..
        } = self;
        let (utilization_source, event_source, driver) =
            utilization::create(initial_util, usage, limit_source.subscribe_forward());
        let driver_task = oe_spawn!("credit-utilization", driver.map(|()| Ok(())));
        let mut keepalive_shutdown = shutdown.signal();
        let keepalive_task = oe_spawn!("credit-limit-static", async move {
            keepalive_shutdown.wait().await;
            drop(limit_producer);
            Ok(())
        });
        CreditRuntime {
            limit_source,
            utilization_source,
            event_source,
            utilization_task: driver_task,
            limit_keepalive_task: keepalive_task,
        }
    }
}

pub struct CreditRuntimeTasks {
    pub utilization: JoinHandle<Result<(), Report>>,
    pub limit_keepalive: JoinHandle<Result<(), Report>>,
}

/// Live handles produced by [`CreditModule::start`]. Sources are
/// exposed via accessor methods while the runtime is live;
/// [`Self::detach`] consumes it at startup's tail to extract the
/// background task handles.
pub struct CreditRuntime {
    limit_source: CreditLimitSource,
    utilization_source: CreditUtilizationSource,
    event_source: CreditEventSource,
    utilization_task: JoinHandle<Result<(), Report>>,
    limit_keepalive_task: JoinHandle<Result<(), Report>>,
}

impl CreditRuntime {
    pub fn limit_source(&self) -> &CreditLimitSource {
        &self.limit_source
    }

    pub fn utilization_source(&self) -> &CreditUtilizationSource {
        &self.utilization_source
    }

    pub fn event_source(&self) -> &CreditEventSource {
        &self.event_source
    }

    /// Consume the runtime, returning the spawned background tasks split by
    /// lifecycle role: utilization is derived, while the limit keepalive is a
    /// source/control-loop node and receives process shutdown directly.
    pub fn detach(self) -> CreditRuntimeTasks {
        CreditRuntimeTasks {
            utilization: self.utilization_task,
            limit_keepalive: self.limit_keepalive_task,
        }
    }
}

/// Build the [`CreditModule`]. Seeds the limits from `limits_cfg`
/// (falling back to the per-field defaults), creates the limit watch,
/// stores the optional sink config for later binding, and returns the
/// configured [`CreditCalculator`] for the server link.
pub fn init(
    Config {
        limits: limits_cfg,
        utilization: utilization_cfg,
        calculator: calculator_config,
    }: Config,
) -> (CreditModule, CreditCalculator) {
    let initial_limits = CreditLimitState {
        rest: CreditLimit::new(limits_cfg.rest),
        breaks: CreditLimit::new(limits_cfg.breaks),
        day: CreditLimit::new(limits_cfg.day),
    };
    let (limit_source, limit_producer) = limit::create(initial_limits);
    let module = CreditModule {
        utilization_cfg,
        limit_producer,
        limit_source,
    };
    (module, CreditCalculator::new(calculator_config))
}

fn bind_sink<T: EndpointConfig>(
    binder: &mut Binder<T>,
    field: &str,
    label: Option<&str>,
    initial: f64,
) -> Result<Option<AnalogOutProducer>, Report> {
    let Some(label) = label else {
        return Ok(None);
    };
    let label_handle = resolve_endpoint(binder, "credit.utilization", field, label)?;
    let producer = binder.analog_out(label_handle, initial).context(format!(
        "Failed to bind credit.utilization.{field} as output"
    ))?;
    Ok(Some(producer))
}

fn resolve_endpoint<T: EndpointConfig>(
    binder: &Binder<T>,
    section: &str,
    field: &str,
    label: &str,
) -> Result<EndpointLabel, Report> {
    binder
        .labels()
        .get(label)
        .ok_or_else(|| report!("{section}.{field} references unknown control '{label}'"))
}
