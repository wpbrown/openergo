use bachelor::broadcast::spmc::broadcast;
use bachelor::channel::mpsc::MpscChannelConsumer;
use clap::Parser;
use futures::StreamExt;
use futures::future::{Either, select};
use openergo_server::device_events::{DeviceFilter, DeviceMatcher};
use openergo_server::{config, device_events, dwell_click, server, usage};
use rootcause::prelude::*;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use tokio::net::UnixListener;
use tokio::task::spawn_local;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    user: Option<String>,

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

    let (device_filter, socket_user, socket_group) = match args.config {
        Some(path) => {
            let config = config::Config::load(&path).context("Failed to load configuration")?;
            let config::Config { socket, devices } = config;
            let filter = device_filter_from_config(devices);
            let (cfg_user, cfg_group) = socket.map(|s| (s.user, s.group)).unwrap_or_default();
            let user = args.user.or(cfg_user);
            (filter, user, cfg_group)
        }
        None => (DeviceFilter::default(), args.user, None),
    };

    rt.block_on(async move {
        let listener = listener::create_listener(socket_user.as_deref(), socket_group.as_deref())
            .context("Failed to create socket listener")?;
        run(listener, device_filter).await
    })
}

const EVENT_BROADCAST_CAPACITY: NonZeroUsize =
    NonZeroUsize::new(32).expect("broadcast capacity must be non-zero");

async fn run(listener: UnixListener, device_filter: DeviceFilter) -> Result<(), Report> {
    // Device Events
    let (device_events, device_events_driver) = device_events::create(device_filter);
    let device_events_task = spawn_local(device_events_driver.run());

    let (device_events_sink, device_events_source) = broadcast(EVENT_BROADCAST_CAPACITY);
    spawn_local(device_events.map(Ok).forward(device_events_sink));

    // Dwell Click
    let (dwell_controller, click_events, dwell_driver) =
        dwell_click::create(device_events_source.subscribe());
    let dwell_task = spawn_local(dwell_driver.run());

    // Usage Events
    let (usage_events, usage_driver) = usage::create(
        usage::DragConfig::default(),
        device_events_source.subscribe(),
    );
    spawn_local(usage_driver.run());

    // IPC Server
    let (server_events, ipc_server) = server::create(listener, usage_events, click_events);
    spawn_local(ipc_server.run());

    // Run until commands channel closes or dwell driver fails
    spawn_local(forward_commands(server_events, dwell_controller));

    match select(device_events_task, dwell_task).await {
        Either::Left((result, _)) => result.expect("task paniced"),
        Either::Right((result, _)) => result.expect("task paniced"),
    }
}

async fn forward_commands(
    mut commands: MpscChannelConsumer<server::ClientCommand>,
    mut dwell_controller: dwell_click::Controller,
) {
    while let Ok(cmd) = commands.recv().await {
        match cmd {
            server::ClientCommand::ConfigureDwellClick(config) => {
                log::info!(
                    "Received new dwell click configuration from client: {:?}",
                    config
                );
                dwell_controller.reconfigure(config).await;
            }
            server::ClientCommand::PauseAutoClick => {
                log::info!("Auto-click paused by client");
                dwell_controller.pause().await;
            }
            server::ClientCommand::ResumeAutoClick => {
                log::info!("Auto-click resumed by client");
                dwell_controller.resume().await;
            }
        }
    }
}

fn device_filter_from_config(devices: Option<config::DevicesConfig>) -> DeviceFilter {
    let Some(devices) = devices else {
        return DeviceFilter::default();
    };

    let convert_matchers = |matchers: Vec<config::DeviceMatcher>| -> Vec<DeviceMatcher> {
        matchers
            .into_iter()
            .map(|m| DeviceMatcher {
                path: m.path.map(PathBuf::from),
                model: m.model,
                model_id: m.model_id,
                vendor_id: m.vendor_id,
                serial: m.serial,
                bus: m.bus,
            })
            .collect()
    };

    DeviceFilter::new(
        devices.auto_detect(),
        devices.include.map(convert_matchers).unwrap_or_default(),
        devices.exclude.map(convert_matchers).unwrap_or_default(),
    )
}

mod listener {
    use rootcause::prelude::*;
    use std::os::unix::{fs, net};
    use std::path::PathBuf;
    use tokio::net::UnixListener;

    type SocketOwner = (Option<u32>, Option<u32>);
    type SocketConfig = (PathBuf, SocketOwner);

    fn resolve_group(group_str: &str) -> Result<u32, Report> {
        if let Ok(gid) = group_str.parse::<u32>() {
            return Ok(gid);
        }
        users::get_group_by_name(group_str)
            .map(|g| g.gid())
            .ok_or_else(|| report!("Group not found"))
            .attach(format!("group: {group_str}"))
    }

    fn get_socket_config(user: Option<&str>, group: Option<&str>) -> Result<SocketConfig, Report> {
        match user {
            Some(user_str) => {
                let user = if let Ok(uid) = user_str.parse::<u32>() {
                    users::get_user_by_uid(uid)
                } else {
                    users::get_user_by_name(user_str)
                }
                .ok_or_else(|| report!("User not found"))
                .attach(format!("user: {user_str}"))?;

                let uid = user.uid();
                let gid = match group {
                    Some(g) => resolve_group(g)?,
                    None => user.primary_group_id(),
                };
                let path = PathBuf::from(format!("/run/user/{}/openergo.sock", uid));
                Ok((path, (Some(uid), Some(gid))))
            }
            None => {
                let gid = group.map(resolve_group).transpose()?;
                Ok((PathBuf::from("/run/openergo.sock"), (None, gid)))
            }
        }
    }

    pub fn create_listener(
        user: Option<&str>,
        group: Option<&str>,
    ) -> Result<UnixListener, Report> {
        let (socket_path, (uid, gid)) = get_socket_config(user, group)?;
        log::trace!(
            "socket_path: {:?}, uid: {:?}, gid: {:?}",
            socket_path,
            uid,
            gid
        );

        let _ = std::fs::remove_file(&socket_path);
        let listener = net::UnixListener::bind(&socket_path)
            .context("Failed to bind socket")
            .attach(format!("socket_path: {}", socket_path.display()))?;

        if uid.is_some() || gid.is_some() {
            fs::chown(&socket_path, uid, gid).context("Failed to set socket ownership")?;
        }

        listener.set_nonblocking(true)?;
        Ok(UnixListener::from_std(listener)?)
    }
}
