use crate::config;
use crate::device_events::{DeviceFilter, DeviceLabel, DeviceLabelStore, DeviceMatcher};
use crate::usage;
use crate::usage::key_hand::{KeyHand, KeyHandClassifier, KeyHandProfile, KeyHandUsageConfig};
use evdev::KeyCode;
use rootcause::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::str::FromStr;

pub struct RuntimeConfig {
    pub device_filter: DeviceFilter,
    pub label_store: Rc<DeviceLabelStore>,
    pub usage_config: usage::UsageConfig,
    pub socket_path: PathBuf,
    pub socket_user: Option<String>,
    pub socket_group: Option<String>,
    pub dwell_click_enabled: bool,
}

/// Returns true if `label` is a valid friendly device label: non-empty and
/// composed only of ASCII alphanumerics, `_`, or `-`.
fn is_valid_label(label: &str) -> bool {
    !label.is_empty()
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn device_matcher_is_empty(matcher: &config::DeviceMatcher) -> bool {
    matcher.path.is_none()
        && matcher.name.is_none()
        && matcher.model.is_none()
        && matcher.model_id.is_none()
        && matcher.vendor_id.is_none()
        && matcher.serial.is_none()
        && matcher.bus.is_none()
}

pub fn validate(config: &config::ConfigFile) -> Result<(), Report> {
    if let Some(devices) = &config.devices {
        let validate_matchers = |matchers: &HashMap<String, config::DeviceMatcher>,
                                 section: &str|
         -> Result<(), Report> {
            for (label, matcher) in matchers {
                if !is_valid_label(label) {
                    bail!(
                        "devices.{section} key {label:?} is not a valid label \
                             (must be non-empty ASCII alphanumerics, '_' or '-')"
                    );
                }
                if device_matcher_is_empty(matcher) {
                    bail!("devices.{section}.{label} has no fields set");
                }
            }
            Ok(())
        };

        if let Some(include) = &devices.include {
            validate_matchers(include, "include")?;
        }
        if let Some(exclude) = &devices.exclude {
            validate_matchers(exclude, "exclude")?;
        }

        if !devices.auto_detect() && devices.include.as_ref().is_none_or(HashMap::is_empty) {
            bail!(
                "auto_detect is false and no include rules are set; \
                 no devices would be monitored"
            );
        }
    }

    if let Some(usage) = &config.usage {
        if let Some(exclude) = &usage.exclude {
            for label in exclude {
                if !is_valid_label(label) {
                    bail!(
                        "usage.exclude entry {label:?} is not a valid label \
                         (must be non-empty ASCII alphanumerics, '_' or '-')"
                    );
                }
            }
        }

        if let Some(devices) = &usage.devices {
            for label in devices.keys() {
                if !is_valid_label(label) {
                    bail!(
                        "usage.devices key {label:?} is not a valid label \
                         (must be non-empty ASCII alphanumerics, '_' or '-')"
                    );
                }
            }
        }
    }

    Ok(())
}

pub fn instantiate(
    args: config::ConfigArgs,
    file_config: config::ConfigFile,
) -> Result<RuntimeConfig, Report> {
    validate(&file_config)?;

    let config::ConfigFile {
        socket,
        devices,
        dwell_click,
        usage,
    } = file_config;
    let dwell_click_enabled = dwell_click.is_some_and(|dc| dc.allow());

    let (device_filter, label_store) = device_filter_from_config(devices);
    let usage_config = usage_config_from_sources(usage, &label_store)?;
    let (cfg_socket_path, cfg_user, cfg_group) = match socket {
        Some(config::SocketConfig { path, user, group }) => (path, user, group),
        None => (None, None, None),
    };
    let socket_path = args
        .socket_path
        .or(cfg_socket_path)
        .unwrap_or_else(|| PathBuf::from(shared::socket::DEFAULT_SERVER_SOCKET_PATH));
    let socket_user = args.socket_user.or(cfg_user);

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
    let Some(config::UsageConfig {
        exclude: exclude_labels,
        default_pointer_hand,
        devices,
    }) = section
    else {
        bail!("missing [usage] section; usage.default_pointer_hand is required");
    };

    for label in exclude_labels.unwrap_or_default() {
        let resolved = resolve_usage_device_label(label_store, &label, "usage.exclude", "entry")?;
        if !exclude.contains(&resolved) {
            exclude.push(resolved);
        }
    }

    let default_pointer_hand = pointer_hand_from_config_value(default_pointer_hand);
    let (key_hand, pointer_hand) =
        usage_device_configs_from_source(devices, label_store, default_pointer_hand)?;
    Ok(usage::UsageConfig {
        exclude,
        key_hand,
        pointer_hand,
    })
}

fn usage_device_configs_from_source(
    section: Option<HashMap<String, config::DeviceUsageConfig>>,
    label_store: &DeviceLabelStore,
    default_pointer_hand: usage::PointerHand,
) -> Result<(KeyHandUsageConfig, usage::PointerHandUsageConfig), Report> {
    let mut device_profiles = Vec::new();
    let mut device_hands = Vec::new();
    if let Some(devices) = section {
        for (label, device_config) in devices {
            let resolved = resolve_usage_device_label(label_store, &label, "usage.devices", "key")?;
            let config::DeviceUsageConfig {
                hand,
                key_profile,
                key_overrides,
            } = device_config;

            if let Some(hand) = hand {
                device_hands.push((resolved, pointer_hand_from_config_value(hand)));
            }

            let mut device_profile = key_hand_profile_for_device(hand, key_profile);
            device_profile = apply_key_hand_overrides(
                device_profile,
                key_overrides,
                &format!("usage.devices.{label}.key_overrides"),
            )?;
            device_profiles.push((resolved, device_profile));
        }
    }

    Ok((
        KeyHandUsageConfig {
            default_profile: KeyHandProfile::default(),
            device_profiles,
        },
        usage::PointerHandUsageConfig {
            default_hand: default_pointer_hand,
            device_hands,
        },
    ))
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
             it must be configured under [devices.include] or [devices.exclude]"
        )
    })
}

fn key_hand_profile_for_device(
    hand: Option<config::HandConfigValue>,
    profile: Option<config::KeyProfileConfigValue>,
) -> KeyHandProfile {
    match profile {
        Some(profile) => key_hand_profile_from_config(profile),
        None => match hand {
            Some(config::HandConfigValue::Left) => KeyHandProfile::Left,
            Some(config::HandConfigValue::Right) => KeyHandProfile::Right,
            None => KeyHandProfile::default(),
        },
    }
}

fn key_hand_profile_from_config(profile: config::KeyProfileConfigValue) -> KeyHandProfile {
    match profile {
        config::KeyProfileConfigValue::AnsiQwerty => {
            KeyHandProfile::UnclassifiedCustom(KeyHandClassifier::ansi_qwerty())
        }
        config::KeyProfileConfigValue::None => KeyHandProfile::Unclassified,
        config::KeyProfileConfigValue::AllLeft => KeyHandProfile::Left,
        config::KeyProfileConfigValue::AllRight => KeyHandProfile::Right,
    }
}

fn apply_key_hand_overrides(
    profile: KeyHandProfile,
    overrides: Option<HashMap<String, config::KeyOverrideValue>>,
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

fn key_hand_from_config_value(value: config::KeyOverrideValue) -> KeyHand {
    match value {
        config::KeyOverrideValue::Left => KeyHand::Left,
        config::KeyOverrideValue::Right => KeyHand::Right,
        config::KeyOverrideValue::Unclassified => KeyHand::Unclassified,
    }
}

fn pointer_hand_from_config_value(value: config::HandConfigValue) -> usage::PointerHand {
    match value {
        config::HandConfigValue::Left => usage::PointerHand::Left,
        config::HandConfigValue::Right => usage::PointerHand::Right,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_usage_config() -> config::UsageConfig {
        config::UsageConfig {
            exclude: None,
            default_pointer_hand: config::HandConfigValue::Right,
            devices: None,
        }
    }

    fn no_args() -> config::ConfigArgs {
        config::ConfigArgs {
            socket_path: None,
            socket_user: None,
        }
    }

    #[test]
    fn devices_include_validates() {
        let config: config::ConfigFile = toml::from_str(
            r#"
            [devices.include.keyboard]
            serial = "Chicony_USB_Keyboard"

            [devices.include.mouse]
            name = "Some Mouse"
            "#,
        )
        .expect("config should parse");

        validate(&config).expect("config should validate");
    }

    #[test]
    fn empty_matcher_rejected() {
        let config: config::ConfigFile = toml::from_str(
            r#"
            [devices.include.keyboard]
            "#,
        )
        .expect("config should parse");

        let err = validate(&config).expect_err("empty matcher must error");
        assert!(
            format!("{err}").contains("devices.include.keyboard"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn invalid_label_rejected() {
        let config: config::ConfigFile = toml::from_str(
            r#"
            [devices.include."bad label"]
            serial = "x"
            "#,
        )
        .expect("config should parse");

        let err = validate(&config).expect_err("invalid label must error");
        assert!(
            format!("{err}").contains("not a valid label"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn empty_label_rejected() {
        let config: config::ConfigFile = toml::from_str(
            r#"
            [devices.include.""]
            serial = "x"
            "#,
        )
        .expect("config should parse");

        let err = validate(&config).expect_err("empty label must error");
        assert!(
            format!("{err}").contains("not a valid label"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn auto_detect_off_with_no_includes_rejected() {
        let config: config::ConfigFile = toml::from_str(
            r#"
            [devices]
            auto_detect = false
            "#,
        )
        .expect("config should parse");

        let err = validate(&config).expect_err("must reject when nothing would be monitored");
        assert!(
            format!("{err}").contains("auto_detect is false"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn usage_device_label_syntax_is_validated() {
        let config: config::ConfigFile = toml::from_str(
            r#"
            [usage]
            default_pointer_hand = "right"

            [usage.devices."bad label"]
            key_profile = "none"
            "#,
        )
        .expect("config should parse");

        let err = validate(&config).expect_err("invalid usage device label should error");
        assert!(
            format!("{err}").contains("usage.devices"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn usage_devices_config_validates() {
        let config: config::ConfigFile = toml::from_str(
            r#"
            [usage]
            default_pointer_hand = "right"

            [usage.devices.left_mouse]
            hand = "left"

            [usage.devices.main_keyboard]
            key_profile = "ansi_qwerty"

            [usage.devices.main_keyboard.key_overrides]
            KEY_SPACE = "left"

            [usage.devices.layer_pad]
            key_profile = "none"

            [usage.devices.layer_pad.key_overrides]
            KEY_F13 = "unclassified"
            "#,
        )
        .expect("config should parse");

        validate(&config).expect("config should validate");
    }

    #[test]
    fn cli_socket_path_overrides_config_socket_path() {
        let runtime = instantiate(
            config::ConfigArgs {
                socket_path: Some(PathBuf::from("/tmp/from-cli.sock")),
                socket_user: None,
            },
            config::ConfigFile {
                socket: Some(config::SocketConfig {
                    path: Some(PathBuf::from("/tmp/from-config.sock")),
                    user: Some("alice".to_string()),
                    group: Some("input".to_string()),
                }),
                dwell_click: None,
                devices: None,
                usage: Some(minimal_usage_config()),
            },
        )
        .expect("runtime config should build");

        assert_eq!(runtime.socket_path, PathBuf::from("/tmp/from-cli.sock"));
        assert_eq!(runtime.socket_user.as_deref(), Some("alice"));
        assert_eq!(runtime.socket_group.as_deref(), Some("input"));
    }

    #[test]
    fn config_socket_path_overrides_default() {
        let runtime = instantiate(
            no_args(),
            config::ConfigFile {
                socket: Some(config::SocketConfig {
                    path: Some(PathBuf::from("/tmp/from-config.sock")),
                    user: None,
                    group: None,
                }),
                dwell_click: None,
                devices: None,
                usage: Some(minimal_usage_config()),
            },
        )
        .expect("runtime config should build");

        assert_eq!(runtime.socket_path, PathBuf::from("/tmp/from-config.sock"));
    }

    #[test]
    fn usage_device_config_defaults_and_overrides_compile() {
        let mut labels = DeviceLabelStore::new();
        let main_keyboard = labels.get_or_intern("main_keyboard");
        let layer_pad = labels.get_or_intern("layer_pad");
        let right_mouse = labels.get_or_intern("right_mouse");

        let usage_config = usage_config_from_sources(
            Some(config::UsageConfig {
                exclude: None,
                default_pointer_hand: config::HandConfigValue::Right,
                devices: Some(HashMap::from([
                    (
                        "main_keyboard".to_string(),
                        config::DeviceUsageConfig {
                            hand: None,
                            key_profile: None,
                            key_overrides: Some(HashMap::from([(
                                "KEY_B".to_string(),
                                config::KeyOverrideValue::Right,
                            )])),
                        },
                    ),
                    (
                        "layer_pad".to_string(),
                        config::DeviceUsageConfig {
                            hand: Some(config::HandConfigValue::Left),
                            key_profile: None,
                            key_overrides: Some(HashMap::from([
                                ("KEY_F13".to_string(), config::KeyOverrideValue::Left),
                                ("KEY_F14".to_string(), config::KeyOverrideValue::Right),
                                (
                                    "KEY_F15".to_string(),
                                    config::KeyOverrideValue::Unclassified,
                                ),
                            ])),
                        },
                    ),
                    (
                        "right_mouse".to_string(),
                        config::DeviceUsageConfig {
                            hand: Some(config::HandConfigValue::Right),
                            key_profile: None,
                            key_overrides: None,
                        },
                    ),
                ])),
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

        let main_profile = usage_config.key_hand.profile_for(main_keyboard);
        assert_eq!(main_profile.classify(KeyCode::KEY_B), KeyHand::Right);

        let layer_profile = usage_config.key_hand.profile_for(layer_pad);
        assert_eq!(layer_profile.classify(KeyCode::KEY_A), KeyHand::Left);
        assert_eq!(layer_profile.classify(KeyCode::KEY_F13), KeyHand::Left);
        assert_eq!(layer_profile.classify(KeyCode::KEY_F14), KeyHand::Right);
        assert_eq!(
            layer_profile.classify(KeyCode::KEY_F15),
            KeyHand::Unclassified
        );

        let right_mouse_profile = usage_config.key_hand.profile_for(right_mouse);
        assert_eq!(
            right_mouse_profile.classify(KeyCode::KEY_PLAYPAUSE),
            KeyHand::Right
        );
        assert_eq!(
            usage_config.pointer_hand.hand_for(layer_pad),
            usage::PointerHand::Left
        );
        assert_eq!(
            usage_config.pointer_hand.hand_for(main_keyboard),
            usage::PointerHand::Right
        );
    }

    #[test]
    fn usage_device_config_accepts_builtin_key_profiles() {
        for (profile, key, expected) in [
            (
                config::KeyProfileConfigValue::AnsiQwerty,
                KeyCode::KEY_A,
                KeyHand::Left,
            ),
            (
                config::KeyProfileConfigValue::None,
                KeyCode::KEY_A,
                KeyHand::Unclassified,
            ),
            (
                config::KeyProfileConfigValue::AllLeft,
                KeyCode::KEY_PLAYPAUSE,
                KeyHand::Left,
            ),
            (
                config::KeyProfileConfigValue::AllRight,
                KeyCode::KEY_PLAYPAUSE,
                KeyHand::Right,
            ),
        ] {
            let mut labels = DeviceLabelStore::new();
            let label = labels.get_or_intern("device");
            let usage_config = usage_config_from_sources(
                Some(config::UsageConfig {
                    exclude: None,
                    default_pointer_hand: config::HandConfigValue::Right,
                    devices: Some(HashMap::from([(
                        "device".to_string(),
                        config::DeviceUsageConfig {
                            hand: None,
                            key_profile: Some(profile),
                            key_overrides: None,
                        },
                    )])),
                }),
                &labels,
            )
            .expect("usage config should compile");

            assert_eq!(
                usage_config.key_hand.profile_for(label).classify(key),
                expected,
                "profile {profile:?} classified {key:?} unexpectedly"
            );
        }
    }

    #[test]
    fn key_overrides_on_constant_profiles_build_custom_map() {
        for (profile, default, override_value, override_hand) in [
            (
                config::KeyProfileConfigValue::AllLeft,
                KeyHand::Left,
                config::KeyOverrideValue::Right,
                KeyHand::Right,
            ),
            (
                config::KeyProfileConfigValue::AllRight,
                KeyHand::Right,
                config::KeyOverrideValue::Left,
                KeyHand::Left,
            ),
        ] {
            let mut labels = DeviceLabelStore::new();
            let label = labels.get_or_intern("device");
            let usage_config = usage_config_from_sources(
                Some(config::UsageConfig {
                    exclude: None,
                    default_pointer_hand: config::HandConfigValue::Right,
                    devices: Some(HashMap::from([(
                        "device".to_string(),
                        config::DeviceUsageConfig {
                            hand: None,
                            key_profile: Some(profile),
                            key_overrides: Some(HashMap::from([
                                ("KEY_A".to_string(), config::KeyOverrideValue::Unclassified),
                                ("KEY_J".to_string(), override_value),
                            ])),
                        },
                    )])),
                }),
                &labels,
            )
            .expect("usage config should compile");

            let compiled_profile = usage_config.key_hand.profile_for(label);
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
    fn key_overrides_on_none_profile_build_custom_map() {
        let mut labels = DeviceLabelStore::new();
        let label = labels.get_or_intern("device");
        let usage_config = usage_config_from_sources(
            Some(config::UsageConfig {
                exclude: None,
                default_pointer_hand: config::HandConfigValue::Right,
                devices: Some(HashMap::from([(
                    "device".to_string(),
                    config::DeviceUsageConfig {
                        hand: None,
                        key_profile: Some(config::KeyProfileConfigValue::None),
                        key_overrides: Some(HashMap::from([
                            ("KEY_F13".to_string(), config::KeyOverrideValue::Left),
                            ("KEY_F14".to_string(), config::KeyOverrideValue::Right),
                        ])),
                    },
                )])),
            }),
            &labels,
        )
        .expect("usage config should compile");

        let compiled_profile = usage_config.key_hand.profile_for(label);
        assert_eq!(
            compiled_profile.classify(KeyCode::KEY_A),
            KeyHand::Unclassified
        );
        assert_eq!(compiled_profile.classify(KeyCode::KEY_F13), KeyHand::Left);
        assert_eq!(compiled_profile.classify(KeyCode::KEY_F14), KeyHand::Right);
    }

    #[test]
    fn unclassified_override_clears_ansi_qwerty_key() {
        let mut labels = DeviceLabelStore::new();
        let label = labels.get_or_intern("keyboard");
        let usage_config = usage_config_from_sources(
            Some(config::UsageConfig {
                exclude: None,
                default_pointer_hand: config::HandConfigValue::Right,
                devices: Some(HashMap::from([(
                    "keyboard".to_string(),
                    config::DeviceUsageConfig {
                        hand: None,
                        key_profile: Some(config::KeyProfileConfigValue::AnsiQwerty),
                        key_overrides: Some(HashMap::from([(
                            "KEY_A".to_string(),
                            config::KeyOverrideValue::Unclassified,
                        )])),
                    },
                )])),
            }),
            &labels,
        )
        .expect("usage config should compile");

        let compiled_profile = usage_config.key_hand.profile_for(label);
        assert_eq!(
            compiled_profile.classify(KeyCode::KEY_A),
            KeyHand::Unclassified
        );
        assert_eq!(compiled_profile.classify(KeyCode::KEY_S), KeyHand::Left);
    }

    #[test]
    fn usage_device_config_requires_known_label() {
        let labels = DeviceLabelStore::new();
        let err = usage_config_from_sources(
            Some(config::UsageConfig {
                exclude: None,
                default_pointer_hand: config::HandConfigValue::Right,
                devices: Some(HashMap::from([(
                    "missing".to_string(),
                    config::DeviceUsageConfig {
                        hand: None,
                        key_profile: Some(config::KeyProfileConfigValue::None),
                        key_overrides: None,
                    },
                )])),
            }),
            &labels,
        )
        .expect_err("unknown device override label should error");

        assert!(
            format!("{err}").contains("usage.devices"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn usage_exclude_requires_known_label() {
        let labels = DeviceLabelStore::new();
        let err = usage_config_from_sources(
            Some(config::UsageConfig {
                exclude: Some(vec!["missing".to_string()]),
                default_pointer_hand: config::HandConfigValue::Right,
                devices: None,
            }),
            &labels,
        )
        .expect_err("unknown usage exclude label should error");

        assert!(
            format!("{err}").contains("usage.exclude"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn excluded_but_known_device_config_is_accepted() {
        let mut labels = DeviceLabelStore::new();
        let ignored = labels.get_or_intern("ignored");
        let usage_config = usage_config_from_sources(
            Some(config::UsageConfig {
                exclude: Some(vec!["ignored".to_string()]),
                default_pointer_hand: config::HandConfigValue::Right,
                devices: Some(HashMap::from([(
                    "ignored".to_string(),
                    config::DeviceUsageConfig {
                        hand: Some(config::HandConfigValue::Left),
                        key_profile: None,
                        key_overrides: None,
                    },
                )])),
            }),
            &labels,
        )
        .expect("excluded but configured usage device should compile");

        assert_eq!(usage_config.exclude, vec![ignored]);
        assert_eq!(
            usage_config.pointer_hand.hand_for(ignored),
            usage::PointerHand::Left
        );
    }

    #[test]
    fn usage_config_requires_usage_section() {
        let labels = DeviceLabelStore::new();
        let err = usage_config_from_sources(None, &labels)
            .expect_err("missing usage section should error");

        assert!(
            format!("{err}").contains("usage.default_pointer_hand"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn key_override_key_must_be_known_evdev_key_code() {
        let mut labels = DeviceLabelStore::new();
        labels.get_or_intern("device");
        let err = usage_config_from_sources(
            Some(config::UsageConfig {
                exclude: None,
                default_pointer_hand: config::HandConfigValue::Right,
                devices: Some(HashMap::from([(
                    "device".to_string(),
                    config::DeviceUsageConfig {
                        hand: None,
                        key_profile: None,
                        key_overrides: Some(HashMap::from([(
                            "KEY_NOT_REAL".to_string(),
                            config::KeyOverrideValue::Left,
                        )])),
                    },
                )])),
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
