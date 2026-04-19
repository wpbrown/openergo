use opentelemetry::global;
use opentelemetry_otlp::{ExportConfig, WithExportConfig};
use opentelemetry_sdk::{
    metrics::{SdkMeterProvider, periodic_reader_with_async_runtime::PeriodicReader},
    runtime::Tokio,
};
use std::time::Duration;

pub fn init() -> SdkMeterProvider {
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_export_config(ExportConfig::default())
        .build()
        .expect("Failed to create OTLP metric exporter");

    let reader = PeriodicReader::builder(exporter, Tokio)
        .with_interval(Duration::from_secs(5))
        .build();

    let provider = SdkMeterProvider::builder().with_reader(reader).build();

    global::set_meter_provider(provider.clone());
    provider
}
