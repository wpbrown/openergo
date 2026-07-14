use crate::activity::ActivityStateConsumer;
use crate::credit::utilization::CreditEventConsumer;
use crate::integration::{
    AnalogIn, AnalogOutProducer, Binder, EndpointConfig, EndpointLabel, EndpointLabelStore,
};
use crate::pain::{self, PainConsumer};
use rootcause::prelude::*;
use shared::oe_spawn;
use shared::spawn::JoinHandle;

pub struct Config {
    pub indicator: Option<String>,
    pub acknowledge: Option<String>,
}

pub struct PainCheckModule {
    indicator: Option<EndpointLabel>,
    acknowledge: Option<EndpointLabel>,
}

impl PainCheckModule {
    pub fn bind_endpoints<T: EndpointConfig>(
        &self,
        binder: &mut Binder<T>,
    ) -> Result<PainCheckBindings, Report> {
        let indicator = self
            .indicator
            .map(|label| bind_indicator(binder, label))
            .transpose()?;
        let acknowledge = self
            .acknowledge
            .map(|label| bind_acknowledge(binder, label))
            .transpose()?;
        Ok(PainCheckBindings {
            indicator,
            acknowledge,
        })
    }

    pub fn start(
        self,
        pain: PainConsumer,
        credit_events: CreditEventConsumer,
        activity: ActivityStateConsumer,
        bindings: PainCheckBindings,
    ) -> JoinHandle<Result<(), Report>> {
        let inputs = pain::check::PainCheckInputs {
            pain,
            credit_events,
            activity,
            acknowledge: bindings.acknowledge,
        };
        oe_spawn!("pain-check", pain::check::run(inputs, bindings.indicator))
    }
}

pub struct PainCheckBindings {
    acknowledge: Option<AnalogIn>,
    indicator: Option<AnalogOutProducer>,
}

pub fn init(
    cfg: Option<Config>,
    endpoint_labels: &'static EndpointLabelStore,
) -> Result<Option<PainCheckModule>, Report> {
    let Some(cfg) = cfg else {
        return Ok(None);
    };

    let Config {
        indicator,
        acknowledge,
    } = cfg;

    let indicator = indicator
        .as_deref()
        .map(|label| resolve_endpoint(endpoint_labels, "indicator", label))
        .transpose()?;
    let acknowledge = acknowledge
        .as_deref()
        .map(|label| resolve_endpoint(endpoint_labels, "acknowledge", label))
        .transpose()?;

    Ok(Some(PainCheckModule {
        indicator,
        acknowledge,
    }))
}

fn resolve_endpoint(
    endpoint_labels: &'static EndpointLabelStore,
    field: &str,
    label: &str,
) -> Result<EndpointLabel, Report> {
    let endpoint = endpoint_labels
        .get(label)
        .ok_or_else(|| report!("pain.check.{field} references unknown control '{label}'"))?;
    Ok(endpoint)
}

fn bind_indicator<T: EndpointConfig>(
    binder: &mut Binder<T>,
    label: EndpointLabel,
) -> Result<AnalogOutProducer, Report> {
    let producer = binder
        .analog_out(label, 0.0)
        .context("Failed to bind pain.check.indicator as output")?;
    Ok(producer)
}

fn bind_acknowledge<T: EndpointConfig>(
    binder: &mut Binder<T>,
    label: EndpointLabel,
) -> Result<AnalogIn, Report> {
    let consumer = binder
        .analog_in(label)
        .context("Failed to bind pain.check.acknowledge as input")?;
    Ok(consumer)
}
