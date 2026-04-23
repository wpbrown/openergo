use bachelor::{
    broadcast::spmc::SpmcBroadcastConsumer,
    error::Closed,
    watch::{MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch},
};
use shared::model::UsageSnapshot;
use shared::protocol::UsageIncrement;

pub fn create(
    usage_rx: SpmcBroadcastConsumer<UsageIncrement>,
    initial_state: UsageSnapshot,
) -> (MpmcWatchRefSource<UsageSnapshot>, impl Future<Output = ()>) {
    let (state_tx, state_source) = mpmc_watch(initial_state);
    let driver = Driver { usage_rx, state_tx };
    (state_source, driver.run())
}

struct Driver {
    usage_rx: SpmcBroadcastConsumer<UsageIncrement>,
    state_tx: MpmcWatchRefProducer<UsageSnapshot>,
}

impl Driver {
    async fn recv_activity(&mut self) -> Result<(), Closed> {
        let Self { usage_rx, state_tx } = self;
        usage_rx
            .recv_ref(|increment| {
                let _ = state_tx.update(|state| *state += &increment.delta);
            })
            .await
    }

    async fn run(mut self) {
        while self.recv_activity().await.is_ok() {}
    }
}

pub mod telemetry {
    use bachelor::watch::MpmcWatchRefConsumer;
    use opentelemetry::global;
    use opentelemetry::metrics::Counter;
    use shared::model::{UsageDelta, UsageSnapshot};

    struct Instruments {
        click_count: Counter<u64>,
        drag_duration_secs: Counter<f64>,
        key_count: Counter<u64>,
        scroll_count: Counter<u64>,
        left_shift_secs: Counter<f64>,
        left_ctrl_secs: Counter<f64>,
        left_alt_secs: Counter<f64>,
        left_meta_secs: Counter<f64>,
        right_shift_secs: Counter<f64>,
        right_ctrl_secs: Counter<f64>,
        right_alt_secs: Counter<f64>,
        right_meta_secs: Counter<f64>,
    }

    impl Instruments {
        fn new() -> Self {
            let meter = global::meter("openergo");
            Self {
                click_count: meter.u64_counter("usage.click_count").build(),
                drag_duration_secs: meter.f64_counter("usage.drag_duration_secs").build(),
                key_count: meter.u64_counter("usage.key_count").build(),
                scroll_count: meter.u64_counter("usage.scroll_count").build(),
                left_shift_secs: meter.f64_counter("usage.left_modifier.shift_secs").build(),
                left_ctrl_secs: meter.f64_counter("usage.left_modifier.ctrl_secs").build(),
                left_alt_secs: meter.f64_counter("usage.left_modifier.alt_secs").build(),
                left_meta_secs: meter.f64_counter("usage.left_modifier.meta_secs").build(),
                right_shift_secs: meter.f64_counter("usage.right_modifier.shift_secs").build(),
                right_ctrl_secs: meter.f64_counter("usage.right_modifier.ctrl_secs").build(),
                right_alt_secs: meter.f64_counter("usage.right_modifier.alt_secs").build(),
                right_meta_secs: meter.f64_counter("usage.right_modifier.meta_secs").build(),
            }
        }

        fn record(&self, delta: &UsageDelta) {
            self.click_count.add(delta.click_count, &[]);
            self.drag_duration_secs
                .add(delta.drag_duration.as_secs_f64(), &[]);
            self.key_count.add(delta.key_count, &[]);
            self.scroll_count.add(delta.scroll_count, &[]);

            let l = &delta.left_modifier_duration;
            self.left_shift_secs.add(l.shift.as_secs_f64(), &[]);
            self.left_ctrl_secs.add(l.ctrl.as_secs_f64(), &[]);
            self.left_alt_secs.add(l.alt.as_secs_f64(), &[]);
            self.left_meta_secs.add(l.meta.as_secs_f64(), &[]);

            let r = &delta.right_modifier_duration;
            self.right_shift_secs.add(r.shift.as_secs_f64(), &[]);
            self.right_ctrl_secs.add(r.ctrl.as_secs_f64(), &[]);
            self.right_alt_secs.add(r.alt.as_secs_f64(), &[]);
            self.right_meta_secs.add(r.meta.as_secs_f64(), &[]);
        }
    }

    pub fn create(consumer: MpmcWatchRefConsumer<UsageSnapshot>) -> impl Future {
        run(consumer)
    }

    async fn run(mut consumer: MpmcWatchRefConsumer<UsageSnapshot>) {
        use std::time::Duration;

        const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);
        const METRIC_EXPORT_INTERVAL_NAME: &str = "OTEL_METRIC_EXPORT_INTERVAL";

        let report_interval = std::env::var(METRIC_EXPORT_INTERVAL_NAME)
            .ok()
            .and_then(|v| v.parse().map(Duration::from_millis).ok())
            .unwrap_or(DEFAULT_INTERVAL);

        let instruments = Instruments::new();
        let mut prev = consumer.get();
        let mut interval = tokio::time::interval(report_interval);

        loop {
            // Wait for at least one change
            if consumer.changed().await.is_err() {
                break;
            }

            let current = consumer.get();
            let delta = current.saturating_delta(&prev);
            instruments.record(&delta);
            prev = current;

            // Rate limit: don't report more often than the export interval
            interval.tick().await;
        }
    }
}
