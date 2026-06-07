use crate::activity::ActivityStateConsumer;
use crate::credit::limit::CreditLimitConsumer;
use crate::pain::PainConsumer;
use crate::telemetry;
use crate::usage::AllUsageConsumer;
use rootcause::prelude::*;
use shared::oe_spawn;
use shared::spawn::JoinHandle;
use tokio::task::spawn_blocking;
use tracing::info;

pub struct TelemetryModule {
    provider: opentelemetry_sdk::metrics::SdkMeterProvider,
    report_usage: bool,
}

impl TelemetryModule {
    /// Spawn the telemetry tail task. If usage reporting is enabled, the task
    /// emits deltas until input closure and then records one final delta.
    /// Provider shutdown happens afterwards in the same task so the SDK flushes
    /// that final observation.
    pub fn start(
        self,
        usage: AllUsageConsumer,
        pain: PainConsumer,
        limits: CreditLimitConsumer,
        activity: ActivityStateConsumer,
    ) -> JoinHandle<Result<(), Report>> {
        let Self {
            provider,
            report_usage,
        } = self;
        oe_spawn!("telemetry", async move {
            if report_usage {
                telemetry::writer::create(usage, pain, limits, activity).await?;
            } else {
                info!("Usage telemetry reporting disabled");
                telemetry::writer::wait_closed(usage, pain, limits, activity).await;
            }

            let shutdown_result = spawn_blocking(move || provider.shutdown())
                .await
                .expect("OE future can not panic or be canceled");
            shutdown_result.context("failed to shut down OpenTelemetry provider")?;
            Ok(())
        })
    }
}

pub fn init(report_usage: bool) -> Result<TelemetryModule, Report> {
    let provider = telemetry::init().context("failed to initialize telemetry")?;
    Ok(TelemetryModule {
        provider,
        report_usage,
    })
}
