use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Port deltas for each binary's `tokio-console` server, applied as
/// `console_subscriber::Server::DEFAULT_PORT + delta`. Defined here so the
/// values cannot collide between binaries.
pub mod console_port_delta {
    pub const CLIENT: u16 = 0;
    pub const SERVER: u16 = 1;
    pub const CLI: u16 = 2;
}

fn env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

pub fn init_tracing(_console_port_delta: u16) {
    #[cfg(feature = "systemd")]
    if libsystemd::logging::connected_to_journal() {
        match tracing_journald::layer() {
            Ok(layer) => {
                use tracing::info;

                install(layer.with_filter(fmt_filter()), _console_port_delta);
                info!("connected to journald");
                return;
            }
            Err(e) => {
                eprintln!("openergo: failed to init journald logger ({e}); falling back to stderr");
            }
        }
    }

    let fmt_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
            "%Y-%m-%d %I:%M:%S%p".to_string(),
        ))
        .with_filter(fmt_filter());

    install(fmt_layer, _console_port_delta);
}

fn install<L>(primary: L, _console_port_delta: u16)
where
    L: Layer<tracing_subscriber::Registry> + Send + Sync + 'static,
{
    let registry = tracing_subscriber::registry().with(primary);

    #[cfg(feature = "tokio-console")]
    let registry = registry.with(
        console_subscriber::ConsoleLayer::builder()
            .server_addr((
                console_subscriber::Server::DEFAULT_IP,
                console_subscriber::Server::DEFAULT_PORT + _console_port_delta,
            ))
            .spawn(),
    );

    registry.init();
}

#[cfg(not(feature = "tokio-console"))]
fn fmt_filter() -> EnvFilter {
    env_filter()
}

#[cfg(feature = "tokio-console")]
fn fmt_filter() -> tokio_console::FmtFilter {
    tokio_console::FmtFilter::new(env_filter())
}

#[cfg(feature = "tokio-console")]
mod tokio_console {
    use tracing::level_filters::LevelFilter;
    use tracing::subscriber::Interest;
    use tracing::{Metadata, Subscriber};
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::layer::{Context, Filter};
    use tracing_subscriber::registry::LookupSpan;

    pub struct FmtFilter {
        env: EnvFilter,
    }

    impl FmtFilter {
        pub fn new(env: EnvFilter) -> Self {
            Self { env }
        }
    }

    fn is_runtime_target(target: &str) -> bool {
        matches!(target.split("::").next(), Some("tokio" | "runtime"))
    }

    impl<S> Filter<S> for FmtFilter
    where
        S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    {
        fn enabled(&self, meta: &Metadata<'_>, ctx: &Context<'_, S>) -> bool {
            if is_runtime_target(meta.target()) {
                return false;
            }
            Filter::<S>::enabled(&self.env, meta, ctx)
        }

        fn callsite_enabled(&self, meta: &'static Metadata<'static>) -> Interest {
            if is_runtime_target(meta.target()) {
                return Interest::never();
            }
            Filter::<S>::callsite_enabled(&self.env, meta)
        }

        fn max_level_hint(&self) -> Option<LevelFilter> {
            Filter::<S>::max_level_hint(&self.env)
        }
    }
}
