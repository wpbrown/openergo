use bachelor::broadcast::spmc::broadcast;
use bachelor::channel::mpsc::MpscChannelConsumer;
use clap::Parser;
use evdev::KeyCode;
use futures::StreamExt;
use futures::future::{Either, select};
use openergo_server::device_events::{DeviceFilter, DeviceLabel, DeviceLabelStore, DeviceMatcher};
use openergo_server::usage::key_hand::{
    KeyHand, KeyHandClassifier, KeyHandProfile, KeyHandUsageConfig,
};
use openergo_server::{config, device_events, dwell_click, server, usage};
use rootcause::prelude::*;
use shared::oe_spawn;
use shared::shutdown::ShutdownSource;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::pin::pin;
use std::rc::Rc;
use std::str::FromStr;
use tokio::net::UnixListener;
use tracing::info;

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
    shared::tracing_fmt::init_tracing(shared::tracing_fmt::console_port_delta::SERVER);
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
        .unwrap_or_else(|| PathBuf::from(shared::socket::DEFAULT_SERVER_SOCKET_PATH));
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
    let (exclude_labels, key_hand_section) = match section {
        Some(config::UsageConfig { exclude, key_hand }) => (exclude.unwrap_or_default(), key_hand),
        None => (Vec::new(), None),
    };

    for label in exclude_labels {
        let resolved = resolve_usage_device_label(label_store, &label, "usage.exclude", "entry")?;
        if !exclude.contains(&resolved) {
            exclude.push(resolved);
        }
    }

    let key_hand = key_hand_config_from_source(key_hand_section, label_store)?;
    Ok(usage::UsageConfig { exclude, key_hand })
}

fn key_hand_config_from_source(
    section: Option<config::KeyHandConfig>,
    label_store: &DeviceLabelStore,
) -> Result<KeyHandUsageConfig, Report> {
    let Some(config::KeyHandConfig {
        profile,
        overrides,
        devices,
    }) = section
    else {
        return Ok(KeyHandUsageConfig::default());
    };

    let mut default_profile =
        key_hand_profile_from_config(profile.as_deref(), "usage.key_hand.profile")?;
    default_profile =
        apply_key_hand_overrides(default_profile, overrides, "usage.key_hand.overrides")?;

    let mut device_profiles = Vec::new();
    if let Some(devices) = devices {
        for (label, device_config) in devices {
            let resolved =
                resolve_usage_device_label(label_store, &label, "usage.key_hand.devices", "key")?;
            let config::DeviceKeyHandConfig { profile, overrides } = device_config;
            let mut device_profile = match profile {
                Some(profile) => parse_key_hand_profile(
                    &profile,
                    &format!("usage.key_hand.devices.{label}.profile"),
                )?,
                None => default_profile.clone(),
            };
            device_profile = apply_key_hand_overrides(
                device_profile,
                overrides,
                &format!("usage.key_hand.devices.{label}.overrides"),
            )?;
            device_profiles.push((resolved, device_profile));
        }
    }

    Ok(KeyHandUsageConfig {
        default_profile,
        device_profiles,
    })
}

fn resolve_usage_device_label(
    label_store: &DeviceLabelStore,
    label: &str,
    section: &str,
    noun: &str,
) -> Result<DeviceLabel, Report> {
    label_store.get(label).ok_or_else(|| {
        report!(
            "{section} {noun} {label:?} references unknown device label; \
             it must be defined under [devices.include]"
        )
    })
}

fn key_hand_profile_from_config(
    profile: Option<&str>,
    path: &str,
) -> Result<KeyHandProfile, Report> {
    Ok(profile
        .map(|profile| parse_key_hand_profile(profile, path))
        .transpose()?
        .unwrap_or_default())
}

fn parse_key_hand_profile(profile: &str, path: &str) -> Result<KeyHandProfile, Report> {
    match profile {
        "ansi_qwerty" => Ok(KeyHandProfile::UnclassifiedCustom(
            KeyHandClassifier::ansi_qwerty(),
        )),
        "none" => Ok(KeyHandProfile::Unclassified),
        "all_left" => Ok(KeyHandProfile::Left),
        "all_right" => Ok(KeyHandProfile::Right),
        other => {
            bail!(
                "{path} has unsupported value {other:?}; expected \"ansi_qwerty\", \"none\", \"all_left\", or \"all_right\""
            )
        }
    }
}

fn apply_key_hand_overrides(
    profile: KeyHandProfile,
    overrides: Option<HashMap<String, config::KeyHandOverrideValue>>,
    path: &str,
) -> Result<KeyHandProfile, Report> {
    let Some(overrides) = overrides else {
        return Ok(profile);
    };
    if overrides.is_empty() {
        return Ok(profile);
    }

    let default = profile.default_hand();
    let mut classifier = match profile {
        KeyHandProfile::UnclassifiedCustom(classifier)
        | KeyHandProfile::LeftCustom(classifier)
        | KeyHandProfile::RightCustom(classifier) => classifier,
        KeyHandProfile::Unclassified | KeyHandProfile::Left | KeyHandProfile::Right => {
            KeyHandClassifier::new()
        }
    };

    for (key_name, hand) in overrides {
        let key = KeyCode::from_str(&key_name).map_err(|_| {
            report!("{path} override key {key_name:?} is not a known evdev KeyCode")
        })?;
        classifier.set(key, key_hand_from_config_value(hand), default);
    }

    Ok(match default {
        KeyHand::Unclassified => KeyHandProfile::UnclassifiedCustom(classifier),
        KeyHand::Left => KeyHandProfile::LeftCustom(classifier),
        KeyHand::Right => KeyHandProfile::RightCustom(classifier),
    })
}

fn key_hand_from_config_value(value: config::KeyHandOverrideValue) -> KeyHand {
    match value {
        config::KeyHandOverrideValue::Left => KeyHand::Left,
        config::KeyHandOverrideValue::Right => KeyHand::Right,
        config::KeyHandOverrideValue::Other => KeyHand::Unclassified,
    }
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
    use libsystemd::activation::{IsType, receive_descriptors};
    use rootcause::prelude::*;
    use std::fs;
    use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::{fs as unix_fs, net};
    use std::path::Path;
    use tokio::net::UnixListener;
    use tracing::{info, trace};

    type SocketOwner = (Option<u32>, Option<u32>);

    fn try_inherit_systemd_listener() -> Result<Option<UnixListener>, Report> {
        let mut fds = receive_descriptors(true)
            .map_err(|e| report!("Failed to receive systemd file descriptors: {e}"))?;
        let fd = match fds.len() {
            0 => return Ok(None),
            1 => fds.remove(0),
            n => bail!("Expected exactly one inherited systemd socket, got {n}"),
        };
        if !fd.is_unix() {
            bail!("Inherited systemd file descriptor is not a Unix socket");
        }

        // SAFETY: `receive_descriptors` returns owned file descriptors passed
        // by systemd, and `IntoRawFd::into_raw_fd` transfers that ownership to
        // us.
        let owned = unsafe { OwnedFd::from_raw_fd(fd.into_raw_fd()) };
        let listener = net::UnixListener::from(owned);
        listener
            .set_nonblocking(true)
            .context("Failed to set inherited socket nonblocking")?;
        info!("Using inherited systemd socket; local socket bind settings are ignored");
        Ok(Some(UnixListener::from_std(listener)?))
    }

    fn resolve_group(group_str: &str) -> Result<u32, Report> {
        if let Ok(gid) = group_str.parse::<u32>() {
            return Ok(gid);
        }
        uzers::get_group_by_name(group_str)
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
                    uzers::get_user_by_uid(uid)
                } else {
                    uzers::get_user_by_name(user_str)
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
        trace!(
            "socket_path: {:?}, uid: {:?}, gid: {:?}",
            socket_path, uid, gid
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
    fn key_hand_config_defaults_and_overrides_compile() {
        let mut labels = DeviceLabelStore::new();
        let main_keyboard = labels.get_or_intern("main_keyboard");
        let layer_pad = labels.get_or_intern("layer_pad");

        let usage_config = usage_config_from_sources(
            Some(config::UsageConfig {
                exclude: None,
                key_hand: Some(config::KeyHandConfig {
                    profile: None,
                    overrides: Some(HashMap::from([(
                        "KEY_SPACE".to_string(),
                        config::KeyHandOverrideValue::Left,
                    )])),
                    devices: Some(HashMap::from([
                        (
                            "main_keyboard".to_string(),
                            config::DeviceKeyHandConfig {
                                profile: None,
                                overrides: Some(HashMap::from([(
                                    "KEY_B".to_string(),
                                    config::KeyHandOverrideValue::Right,
                                )])),
                            },
                        ),
                        (
                            "layer_pad".to_string(),
                            config::DeviceKeyHandConfig {
                                profile: Some("none".to_string()),
                                overrides: Some(HashMap::from([
                                    ("KEY_F13".to_string(), config::KeyHandOverrideValue::Left),
                                    ("KEY_F14".to_string(), config::KeyHandOverrideValue::Right),
                                ])),
                            },
                        ),
                    ])),
                }),
            }),
            &labels,
        )
        .expect("usage config should compile");

        assert_eq!(
            usage_config
                .key_hand
                .default_profile
                .classify(KeyCode::KEY_B),
            KeyHand::Left
        );
        assert_eq!(
            usage_config
                .key_hand
                .default_profile
                .classify(KeyCode::KEY_SPACE),
            KeyHand::Left
        );

        let main_profile = usage_config.key_hand.profile_for(main_keyboard);
        assert_eq!(main_profile.classify(KeyCode::KEY_SPACE), KeyHand::Left);
        assert_eq!(main_profile.classify(KeyCode::KEY_B), KeyHand::Right);

        let layer_profile = usage_config.key_hand.profile_for(layer_pad);
        assert_eq!(
            layer_profile.classify(KeyCode::KEY_A),
            KeyHand::Unclassified
        );
        assert_eq!(layer_profile.classify(KeyCode::KEY_F13), KeyHand::Left);
        assert_eq!(layer_profile.classify(KeyCode::KEY_F14), KeyHand::Right);
    }

    #[test]
    fn key_hand_config_accepts_builtin_profiles() {
        for (profile, key, expected) in [
            ("ansi_qwerty", KeyCode::KEY_A, KeyHand::Left),
            ("none", KeyCode::KEY_A, KeyHand::Unclassified),
            ("all_left", KeyCode::KEY_PLAYPAUSE, KeyHand::Left),
            ("all_right", KeyCode::KEY_PLAYPAUSE, KeyHand::Right),
        ] {
            let labels = DeviceLabelStore::new();
            let usage_config = usage_config_from_sources(
                Some(config::UsageConfig {
                    exclude: None,
                    key_hand: Some(config::KeyHandConfig {
                        profile: Some(profile.to_string()),
                        overrides: None,
                        devices: None,
                    }),
                }),
                &labels,
            )
            .expect("usage config should compile");

            assert_eq!(
                usage_config.key_hand.default_profile.classify(key),
                expected,
                "profile {profile:?} classified {key:?} unexpectedly"
            );
        }
    }

    #[test]
    fn key_hand_overrides_on_constant_profiles_build_custom_map() {
        for (profile, default, override_value, override_hand) in [
            (
                "all_left",
                KeyHand::Left,
                config::KeyHandOverrideValue::Right,
                KeyHand::Right,
            ),
            (
                "all_right",
                KeyHand::Right,
                config::KeyHandOverrideValue::Left,
                KeyHand::Left,
            ),
        ] {
            let labels = DeviceLabelStore::new();
            let usage_config = usage_config_from_sources(
                Some(config::UsageConfig {
                    exclude: None,
                    key_hand: Some(config::KeyHandConfig {
                        profile: Some(profile.to_string()),
                        overrides: Some(HashMap::from([
                            ("KEY_A".to_string(), config::KeyHandOverrideValue::Other),
                            ("KEY_J".to_string(), override_value),
                        ])),
                        devices: None,
                    }),
                }),
                &labels,
            )
            .expect("usage config should compile");

            let compiled_profile = &usage_config.key_hand.default_profile;
            assert_eq!(compiled_profile.classify(KeyCode::KEY_B), default);
            assert_eq!(compiled_profile.classify(KeyCode::KEY_PLAYPAUSE), default);
            assert_eq!(
                compiled_profile.classify(KeyCode::KEY_A),
                KeyHand::Unclassified
            );
            assert_eq!(compiled_profile.classify(KeyCode::KEY_J), override_hand);
        }
    }

    #[test]
    fn key_hand_device_override_requires_known_label() {
        let labels = DeviceLabelStore::new();
        let err = usage_config_from_sources(
            Some(config::UsageConfig {
                exclude: None,
                key_hand: Some(config::KeyHandConfig {
                    profile: None,
                    overrides: None,
                    devices: Some(HashMap::from([(
                        "missing".to_string(),
                        config::DeviceKeyHandConfig {
                            profile: Some("none".to_string()),
                            overrides: None,
                        },
                    )])),
                }),
            }),
            &labels,
        )
        .expect_err("unknown device override label should error");

        assert!(
            format!("{err}").contains("usage.key_hand.devices"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn key_hand_override_key_must_be_known_evdev_key_code() {
        let labels = DeviceLabelStore::new();
        let err = usage_config_from_sources(
            Some(config::UsageConfig {
                exclude: None,
                key_hand: Some(config::KeyHandConfig {
                    profile: None,
                    overrides: Some(HashMap::from([(
                        "KEY_NOT_REAL".to_string(),
                        config::KeyHandOverrideValue::Left,
                    )])),
                    devices: None,
                }),
            }),
            &labels,
        )
        .expect_err("invalid override key should error");

        assert!(
            format!("{err}").contains("not a known evdev KeyCode"),
            "unexpected error: {err}"
        );
    }
}
