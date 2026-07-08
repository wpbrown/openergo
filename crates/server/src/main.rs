use bachelor::broadcast::spmc::broadcast;
use bachelor::channel::mpsc::MpscChannelConsumer;
use clap::Parser;
use futures::StreamExt;
use futures::future::{Either, select};
use openergo_server::device_events::{DeviceFilter, DeviceLabelStore};
use openergo_server::instantiate::instantiate;
use openergo_server::{config, device_events, dwell_click, listener, server, usage};
use rootcause::prelude::*;
use shared::oe_spawn;
use shared::shutdown::ShutdownSource;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::pin::pin;
use std::rc::Rc;
use tokio::net::UnixListener;
use tracing::info;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    user: Option<String>,

    /// Path to the Unix domain socket. Overrides the config file.
    #[arg(long)]
    socket_path: Option<PathBuf>,

    /// Path to a TOML configuration file.
    #[arg(short, long)]
    config: Option<PathBuf>,
}

fn main() {
    shared::tracing_fmt::init_tracing(shared::tracing_fmt::console_port_delta::SERVER);
    let args = Args::parse();

    if let Err(report) = startup(args) {
        eprintln!("{report}");
        std::process::exit(1);
    }
}

fn startup(args: Args) -> Result<(), Report> {
    let rt = tokio::runtime::LocalRuntime::new().context("Failed to create tokio runtime")?;

    let file_config =
        config::ConfigFile::load(args.config.as_deref()).context("Failed to load configuration")?;

    let runtime_config = instantiate(
        config::ConfigArgs {
            socket_path: args.socket_path,
            socket_user: args.user,
        },
        file_config,
    )?;

    rt.block_on(async move {
        let listener = listener::create_listener(
            &runtime_config.socket_path,
            runtime_config.socket_user.as_deref(),
            runtime_config.socket_group.as_deref(),
        )
        .context("Failed to create socket listener")?;
        // The server is stateless: there is nothing to flush or persist
        // on shutdown, so we just race `run` against the shutdown signal
        // and let everything drop when either side completes.
        let shutdown = ShutdownSource::new()?;
        let mut shutdown_signal = shutdown.signal();
        let run_fut = pin!(run(
            listener,
            runtime_config.device_filter,
            runtime_config.label_store,
            runtime_config.usage_config,
            runtime_config.dwell_click_enabled,
        ));
        match select(run_fut, shutdown_signal.wait()).await {
            Either::Left((result, _)) => result,
            Either::Right(((), _)) => Ok(()),
        }
    })
}

const EVENT_BROADCAST_CAPACITY: NonZeroUsize =
    NonZeroUsize::new(32).expect("broadcast capacity must be non-zero");

async fn run(
    listener: UnixListener,
    device_filter: DeviceFilter,
    label_store: Rc<DeviceLabelStore>,
    usage_config: usage::UsageConfig,
    dwell_click_enabled: bool,
) -> Result<(), Report> {
    // Device Events
    let (device_events, device_events_driver) = device_events::create(device_filter, label_store);
    let device_events_task = oe_spawn!("device-driver", device_events_driver.run());

    let (device_events_sink, device_events_source) = broadcast(EVENT_BROADCAST_CAPACITY);
    oe_spawn!(
        "device-forwarder",
        device_events.map(Ok).forward(device_events_sink)
    );

    // Dwell Click (optional)
    let (dwell_controller, click_events, dwell_task) = if dwell_click_enabled {
        let (controller, events, driver) = dwell_click::create(device_events_source.subscribe());
        let task = oe_spawn!("dwell-driver", driver.run());
        (Some(controller), Some(events), Some(task))
    } else {
        (None, None, None)
    };

    // Usage Events
    let (usage_events, usage_driver) = usage::create(
        usage::DragConfig::default(),
        usage_config,
        device_events_source.subscribe(),
    );
    oe_spawn!("usage-driver", usage_driver.run());

    // IPC Server
    let (server_events, ipc_server) = server::create(listener, usage_events, click_events);
    oe_spawn!("ipc-server", ipc_server.run());

    // Forward dwell click commands if enabled
    if let Some(dwell_controller) = dwell_controller {
        oe_spawn!(
            "dwell-forwarder",
            forward_commands(server_events, dwell_controller)
        );
    }

    match dwell_task {
        Some(dwell_task) => match select(device_events_task, dwell_task).await {
            Either::Left((result, _)) => result,
            Either::Right((result, _)) => result,
        },
        None => device_events_task.await,
    }
}

async fn forward_commands(
    mut commands: MpscChannelConsumer<server::ClientCommand>,
    mut dwell_controller: dwell_click::Controller,
) {
    while let Ok(cmd) = commands.recv().await {
        match cmd {
            server::ClientCommand::ConfigureDwellClick(config) => {
                info!(
                    "Received new dwell click configuration from client: {:?}",
                    config
                );
                dwell_controller.reconfigure(config).await;
            }
            server::ClientCommand::PauseAutoClick => {
                info!("Auto-click paused by client");
                dwell_controller.pause().await;
            }
            server::ClientCommand::ResumeAutoClick => {
                info!("Auto-click resumed by client");
                dwell_controller.resume().await;
            }
        }
    }
}
