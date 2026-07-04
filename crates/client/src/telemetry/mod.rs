mod instruments;
pub mod writer;

use opentelemetry::global;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::metrics::periodic_reader_with_async_runtime::PeriodicReader;
use opentelemetry_sdk::runtime::Runtime;
use rootcause::prelude::*;
use shared::oe_spawn;
use std::time::Duration;

pub fn init() -> Result<SdkMeterProvider, Report> {
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .build()
        .context("failed to create OTLP metric exporter")?;

    let reader = PeriodicReader::builder(exporter, OeTokio).build();

    let resource = Resource::builder()
        .with_service_name("openergo")
        .with_attribute(opentelemetry::KeyValue::new(
            "service.version",
            env!("CARGO_PKG_VERSION"),
        ))
        .build();

    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(resource)
        .build();

    global::set_meter_provider(provider.clone());
    Ok(provider)
}

#[derive(Clone, Copy)]
pub struct OeTokio;

impl Runtime for OeTokio {
    fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        oe_spawn!("otlp", future);
    }

    fn delay(&self, duration: Duration) -> impl Future<Output = ()> + Send + 'static {
        tokio::time::sleep(duration)
    }
}
