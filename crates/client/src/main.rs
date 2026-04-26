use crate::usage::breaks::BreakState;
use crate::usage::daily::DayState;
use crate::usage::rest::RestState;
use bachelor::broadcast::spmc::broadcast;
use bachelor::signal::mpmc_latched;
use clap::Parser;
use futures::FutureExt;
use rootcause::prelude::*;
use shared::spawn::{JoinHandle, oe_spawn};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::pin::pin;

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

    // Shutdown signal: any task that wants to know when to stop subscribes.
    // Install handlers synchronously (the `signal()` builder registers them
    // immediately, unlike `ctrl_c()` which only registers on first poll).
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .context("Failed to install SIGINT handler")?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("Failed to install SIGTERM handler")?;
    let (shutdown_tx, shutdown_source) = mpmc_latched::signal();
    oe_spawn(async move {
        match futures::future::select(pin!(sigint.recv()), pin!(sigterm.recv())).await {
            futures::future::Either::Left(_) => {
                log::info!("SIGINT received, broadcasting shutdown")
            }
            futures::future::Either::Right(_) => {
                log::info!("SIGTERM received, broadcasting shutdown")
            }
        }
        shutdown_tx.notify();
    });

    const USAGE_BROADCAST_CAPACITY: NonZeroUsize =
        NonZeroUsize::new(16).expect("broadcast capacity must be non-zero");

    let (usage_producer, usage_source) = broadcast(USAGE_BROADCAST_CAPACITY);

    // Rest driver
    let (_rest_state, rest_driver) = usage::rest::create(
        usage_source.subscribe(),
        RestState::default(),
        usage::StartupGap::default(),
    );
    let rest_task = oe_spawn(rest_driver);

    // Breaks driver
    let (_break_state, break_driver) = usage::breaks::create(
        usage_source.subscribe(),
        BreakState::default(),
        usage::StartupGap::default(),
    );
    let break_task = oe_spawn(break_driver);

    // Daily driver
    let (_day_state, daily_driver) =
        usage::daily::create(usage_source.subscribe(), DayState::default());
    let daily_task = oe_spawn(daily_driver);

    // All-time usage driver
    let (all_usage_source, all_usage_driver) =
        usage::all::create(usage_source.subscribe(), Default::default());
    let all_usage_task = oe_spawn(all_usage_driver.map(|()| Ok(())));

    // Telemetry driver (optional)
    if telemetry.is_some_and(|t| t.report_usage()) {
        oe_spawn(usage::all::telemetry::create(
            all_usage_source.subscribe_forward(),
        ));
    } else {
        log::info!("Usage telemetry reporting disabled");
    }

    // Reconnect loop owns the usage_producer; when it returns the producer
    // drops, closing the broadcast and letting the rest/break/all drivers
    // exit cleanly.
    let reconnect_task = oe_spawn(client::reconnect_loop(
        args.server_socket_path.clone(),
        usage_producer,
        shutdown_source.subscribe(),
    ));

    // Wait for all fallible tasks. Return the first error immediately.
    // all_usage_driver is infallible and will exit on broadcast Closed.
    await_fallible(vec![
        reconnect_task,
        rest_task,
        break_task,
        daily_task,
        all_usage_task,
    ])
    .await
}

/// Awaits the given fallible tasks. Returns the first error encountered
/// without waiting for the rest. Returns `Ok(())` only if every task
/// completes successfully.
async fn await_fallible(mut tasks: Vec<JoinHandle<Result<(), Report>>>) -> Result<(), Report> {
    while !tasks.is_empty() {
        let (result, _idx, remaining) = futures::future::select_all(tasks).await;
        tasks = remaining;
        match result {
            Ok(()) => {}
            Err(report) => return Err(report),
        }
    }
    Ok(())
}
