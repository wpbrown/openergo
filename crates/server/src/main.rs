use bachelor::broadcast::spmc::broadcast;
use bachelor::channel::mpsc::MpscChannelConsumer;
use clap::Parser;
use futures::StreamExt;
use futures::future::{Either, select};
use openergo_server::device_events::{DeviceFilter, DeviceLabelStore, DeviceMatcher};
use openergo_server::{config, device_events, dwell_click, server, usage};
use rootcause::prelude::*;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::rc::Rc;
use tokio::net::UnixListener;
use tokio::task::spawn_local;

const DEFAULT_SOCKET_PATH: &str = "/run/openergo.sock";
const DEFAULT_SOCKET_MODE: u32 = 0o660;

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

struct RuntimeConfig {
    device_filter: DeviceFilter,
    label_store: Rc<DeviceLabelStore>,
    usage_config: usage::UsageConfig,
    socket_path: PathBuf,
    socket_user: Option<String>,
    socket_group: Option<String>,
    dwell_click_enabled: bool,
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

    let file_config = match args.config.as_ref() {
        Some(path) => Some(config::Config::load(path).context("Failed to load configuration")?),
        None => None,
    };
    let runtime_config = runtime_config_from_sources(args, file_config)?;

    rt.block_on(async move {
        let listener = listener::create_listener(
            &runtime_config.socket_path,
            runtime_config.socket_user.as_deref(),
            runtime_config.socket_group.as_deref(),
        )
        .context("Failed to create socket listener")?;
        run(
            listener,
            runtime_config.device_filter,
            runtime_config.label_store,
            runtime_config.usage_config,
            runtime_config.dwell_click_enabled,
        )
        .await
    })
}

fn runtime_config_from_sources(
    args: Args,
    file_config: Option<config::Config>,
) -> Result<RuntimeConfig, Report> {
    let (socket, devices, usage_section, dwell_click_enabled) = match file_config {
        Some(config::Config {
            socket,
            devices,
            dwell_click,
            usage,
        }) => (
            socket,
            devices,
            usage,
            dwell_click.is_some_and(|dc| dc.allow()),
        ),
        None => (None, None, None, false),
    };

    let (device_filter, label_store) = device_filter_from_config(devices);
    let usage_config = usage_config_from_sources(usage_section, &label_store)?;
    let (cfg_socket_path, cfg_user, cfg_group) = match socket {
        Some(config::SocketConfig { path, user, group }) => (path, user, group),
        None => (None, None, None),
    };
    let socket_path = args
        .socket_path
        .or(cfg_socket_path)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET_PATH));
    let socket_user = args.user.or(cfg_user);

    Ok(RuntimeConfig {
        device_filter,
        label_store,
        usage_config,
        socket_path,
        socket_user,
        socket_group: cfg_group,
        dwell_click_enabled,
    })
}

fn usage_config_from_sources(
    section: Option<config::UsageConfig>,
    label_store: &DeviceLabelStore,
) -> Result<usage::UsageConfig, Report> {
    let mut exclude = Vec::new();
    if let Some(config::UsageConfig {
        exclude: Some(labels),
    }) = section
    {
        for label in labels {
            let resolved = label_store.get(&label).ok_or_else(|| {
                report!(
                    "usage.exclude references unknown device label {label:?}; \
                     it must be defined under [devices.include]"
                )
            })?;
            if !exclude.contains(&resolved) {
                exclude.push(resolved);
            }
        }
    }
    Ok(usage::UsageConfig { exclude })
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
    let device_events_task = spawn_local(device_events_driver.run());

    let (device_events_sink, device_events_source) = broadcast(EVENT_BROADCAST_CAPACITY);
    spawn_local(device_events.map(Ok).forward(device_events_sink));

    // Dwell Click (optional)
    let (dwell_controller, click_events, dwell_task) = if dwell_click_enabled {
        let (controller, events, driver) = dwell_click::create(device_events_source.subscribe());
        let task = spawn_local(driver.run());
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
    spawn_local(usage_driver.run());

    // IPC Server
    let (server_events, ipc_server) = server::create(listener, usage_events, click_events);
    spawn_local(ipc_server.run());

    // Forward dwell click commands if enabled
    if let Some(dwell_controller) = dwell_controller {
        spawn_local(forward_commands(server_events, dwell_controller));
    }

    match dwell_task {
        Some(dwell_task) => match select(device_events_task, dwell_task).await {
            Either::Left((result, _)) => result.expect("task panicked"),
            Either::Right((result, _)) => result.expect("task panicked"),
        },
        None => device_events_task.await.expect("task panicked"),
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

fn device_filter_from_config(
    devices: Option<config::DevicesConfig>,
) -> (DeviceFilter, Rc<DeviceLabelStore>) {
    let mut label_store = DeviceLabelStore::new();
    let auto_detect_label = label_store.auto_detect();
    let (auto_detect, include, exclude) = match devices {
        Some(devices) => {
            let convert = |matchers: HashMap<String, config::DeviceMatcher>,
                           store: &mut DeviceLabelStore|
             -> Vec<DeviceMatcher> {
                matchers
                    .into_iter()
                    .map(|(label, m)| DeviceMatcher {
                        label: store.get_or_intern(&label),
                        path: m.path.map(PathBuf::from),
                        name: m.name,
                        model: m.model,
                        model_id: m.model_id,
                        vendor_id: m.vendor_id,
                        serial: m.serial,
                        bus: m.bus,
                    })
                    .collect()
            };
            let auto_detect = devices.auto_detect();
            let include = devices
                .include
                .map(|m| convert(m, &mut label_store))
                .unwrap_or_default();
            let exclude = devices
                .exclude
                .map(|m| convert(m, &mut label_store))
                .unwrap_or_default();
            (auto_detect, include, exclude)
        }
        None => (true, Vec::new(), Vec::new()),
    };

    let filter = DeviceFilter::new(auto_detect, auto_detect_label, include, exclude);
    (filter, Rc::new(label_store))
}

mod listener {
    use super::DEFAULT_SOCKET_MODE;
    use rootcause::prelude::*;
    use std::env;
    use std::fs;
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::{fs as unix_fs, net};
    use std::path::Path;
    use tokio::net::UnixListener;

    const SYSTEMD_FIRST_LISTEN_FD: i32 = 3;

    type SocketOwner = (Option<u32>, Option<u32>);

    fn try_inherit_systemd_listener() -> Result<Option<UnixListener>, Report> {
        let Some(listen_pid) = env::var_os("LISTEN_PID") else {
            return Ok(None);
        };
        let Some(listen_fds) = env::var_os("LISTEN_FDS") else {
            return Ok(None);
        };

        let Ok(listen_pid) = listen_pid.to_string_lossy().parse::<u32>() else {
            return Ok(None);
        };
        if listen_pid != std::process::id() {
            return Ok(None);
        }

        let listen_fds = listen_fds
            .to_string_lossy()
            .parse::<u32>()
            .context("Failed to parse LISTEN_FDS")?;
        if listen_fds == 0 {
            return Ok(None);
        }
        if listen_fds != 1 {
            bail!("Expected exactly one inherited systemd socket, got {listen_fds}");
        }

        let listener = unsafe { net::UnixListener::from_raw_fd(SYSTEMD_FIRST_LISTEN_FD) };
        listener
            .set_nonblocking(true)
            .context("Failed to set inherited socket nonblocking")?;
        log::info!("Using inherited systemd socket; local socket bind settings are ignored");
        Ok(Some(UnixListener::from_std(listener)?))
    }

    fn resolve_group(group_str: &str) -> Result<u32, Report> {
        if let Ok(gid) = group_str.parse::<u32>() {
            return Ok(gid);
        }
        users::get_group_by_name(group_str)
            .map(|g| g.gid())
            .ok_or_else(|| report!("Group not found"))
            .attach(format!("group: {group_str}"))
    }

    fn resolve_socket_owner(
        user: Option<&str>,
        group: Option<&str>,
    ) -> Result<SocketOwner, Report> {
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
                Ok((Some(uid), Some(gid)))
            }
            None => {
                let gid = group.map(resolve_group).transpose()?;
                Ok((None, gid))
            }
        }
    }

    pub fn create_listener(
        socket_path: &Path,
        user: Option<&str>,
        group: Option<&str>,
    ) -> Result<UnixListener, Report> {
        if let Some(listener) = try_inherit_systemd_listener()? {
            return Ok(listener);
        }

        let (uid, gid) = resolve_socket_owner(user, group)?;
        log::trace!(
            "socket_path: {:?}, uid: {:?}, gid: {:?}",
            socket_path,
            uid,
            gid
        );

        let _ = fs::remove_file(socket_path);
        let listener = net::UnixListener::bind(socket_path)
            .context("Failed to bind socket")
            .attach(format!("socket_path: {}", socket_path.display()))?;

        fs::set_permissions(
            socket_path,
            std::fs::Permissions::from_mode(DEFAULT_SOCKET_MODE),
        )
        .context("Failed to set socket mode")?;

        if uid.is_some() || gid.is_some() {
            unix_fs::chown(socket_path, uid, gid).context("Failed to set socket ownership")?;
        }

        listener.set_nonblocking(true)?;
        Ok(UnixListener::from_std(listener)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_socket_path_overrides_config_socket_path() {
        let runtime = runtime_config_from_sources(
            Args {
                user: None,
                socket_path: Some(PathBuf::from("/tmp/from-cli.sock")),
                config: None,
            },
            Some(config::Config {
                socket: Some(config::SocketConfig {
                    path: Some(PathBuf::from("/tmp/from-config.sock")),
                    user: Some("alice".to_string()),
                    group: Some("input".to_string()),
                }),
                dwell_click: None,
                devices: None,
                usage: None,
            }),
        )
        .expect("runtime config should build");

        assert_eq!(runtime.socket_path, PathBuf::from("/tmp/from-cli.sock"));
        assert_eq!(runtime.socket_user.as_deref(), Some("alice"));
        assert_eq!(runtime.socket_group.as_deref(), Some("input"));
    }

    #[test]
    fn config_socket_path_overrides_default() {
        let runtime = runtime_config_from_sources(
            Args {
                user: None,
                socket_path: None,
                config: None,
            },
            Some(config::Config {
                socket: Some(config::SocketConfig {
                    path: Some(PathBuf::from("/tmp/from-config.sock")),
                    user: None,
                    group: None,
                }),
                dwell_click: None,
                devices: None,
                usage: None,
            }),
        )
        .expect("runtime config should build");

        assert_eq!(runtime.socket_path, PathBuf::from("/tmp/from-config.sock"));
    }

    #[test]
    fn default_socket_path_is_used_when_no_override_is_set() {
        let runtime = runtime_config_from_sources(
            Args {
                user: None,
                socket_path: None,
                config: None,
            },
            None,
        )
        .expect("runtime config should build");

        assert_eq!(runtime.socket_path, PathBuf::from(DEFAULT_SOCKET_PATH));
    }
}
