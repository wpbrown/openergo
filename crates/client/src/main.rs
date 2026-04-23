use crate::usage::rest::RestState;
use bachelor::broadcast::spmc::broadcast;
use clap::Parser;
use futures::future::{Either, select};
use rootcause::prelude::*;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::pin::pin;
use tokio::task::spawn_local;

mod click;
mod client;
mod config;
mod telemetry;
mod usage;

const DEFAULT_SERVER_SOCKET_PATH: &str = "/run/openergo.sock";

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the server's Unix domain socket.
    #[arg(long, default_value = DEFAULT_SERVER_SOCKET_PATH)]
    server_socket_path: PathBuf,

    /// Path to a TOML configuration file.
    #[arg(short, long)]
    config: Option<PathBuf>,
}

fn main() {
    env_logger::init();
    let args = Args::parse();

    if let Err(report) = startup(args) {
        eprintln!("{report}");
        std::process::exit(1);
    }
}

fn startup(args: Args) -> Result<(), Report> {
    let rt = tokio::runtime::LocalRuntime::new().context("Failed to create tokio runtime")?;
    rt.block_on(run(args))
}

async fn run(args: Args) -> Result<(), Report> {
    let config_path = args.config.unwrap_or_else(config::Config::default_path);
    let config = if config_path.exists() {
        config::Config::load(&config_path).context("Failed to load configuration")?
    } else {
        log::info!(
            "No config file found at {}, using defaults",
            config_path.display()
        );
        config::Config::default()
    };
    let telemetry = config.telemetry();

    let _meter_provider = if telemetry.is_some_and(|t| t.enabled()) {
        Some(telemetry::init().context("Failed to initialize telemetry")?)
    } else {
        log::info!("Telemetry disabled by config");
        None
    };

    log::info!("Using socket path: {}", args.server_socket_path.display());

    const USAGE_BROADCAST_CAPACITY: NonZeroUsize =
        NonZeroUsize::new(16).expect("broadcast capacity must be non-zero");

    let (usage_producer, usage_source) = broadcast(USAGE_BROADCAST_CAPACITY);

    // Rest driver
    let (_rest_state, rest_driver) = usage::rest::create(
        usage_source.subscribe(),
        RestState::default(),
        usage::StartupGap::default(),
    );
    let rest_task = spawn_local(rest_driver);

    // All-time usage driver
    let (all_usage_source, all_usage_driver) =
        usage::all::create(usage_source.subscribe(), Default::default());
    let all_usage_task = spawn_local(all_usage_driver);

    // Telemetry driver (optional)
    if telemetry.is_some_and(|t| t.report_usage()) {
        spawn_local(usage::all::telemetry::create(
            all_usage_source.subscribe_forward(),
        ));
    } else {
        log::info!("Usage telemetry reporting disabled");
    }

    // Reconnect loop takes ownership of usage_producer. When it returns
    // (on ctrl_c), the producer is dropped, closing the broadcast channel.
    client::reconnect_loop(&args.server_socket_path, usage_producer).await?;

    // Drivers see Closed and exit their loops. If the rest driver
    // returns an error, propagate it.
    match select(pin!(rest_task), pin!(all_usage_task)).await {
        Either::Left((Ok(result), _)) => result,
        Either::Right((Ok(()), _)) => Ok(()),
        // panic=abort: JoinError can only be a cancellation, not a panic
        Either::Left((Err(e), _)) | Either::Right((Err(e), _)) => {
            bail!("Driver task cancelled: {e}")
        }
    }
}

