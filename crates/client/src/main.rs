use crate::usage::rest::RestState;
use bachelor::broadcast::spmc::broadcast;
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

fn main() {
    env_logger::init();

    if let Err(report) = startup() {
        eprintln!("{report}");
        std::process::exit(1);
    }
}

fn startup() -> Result<(), Report> {
    let rt = tokio::runtime::LocalRuntime::new().context("Failed to create tokio runtime")?;
    rt.block_on(run())
}

fn find_socket_path() -> PathBuf {
    let uid = users::get_current_uid();
    let user_path = PathBuf::from(format!("/run/user/{uid}/openergo.sock"));
    if user_path.exists() {
        user_path
    } else {
        PathBuf::from("/run/openergo.sock")
    }
}

async fn run() -> Result<(), Report> {
    let config = config::Config::load()?;
    let telemetry = config.telemetry();

    let _meter_provider = if telemetry.is_some_and(|t| t.enabled()) {
        Some(telemetry::init().context("Failed to initialize telemetry")?)
    } else {
        log::info!("Telemetry disabled by config");
        None
    };

    let socket_path = find_socket_path();
    log::info!("Using socket path: {}", socket_path.display());

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
    client::reconnect_loop(&socket_path, usage_producer).await?;

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
