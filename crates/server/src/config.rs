use rootcause::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG_PATH: &str = "/etc/openergo.toml";

fn load_path(path: Option<&Path>) -> &Path {
    path.unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub socket: Option<SocketConfig>,
    pub dwell_click: Option<DwellClickConfig>,
    pub devices: Option<DevicesConfig>,
    pub usage: Option<UsageConfig>,
}

pub struct ConfigArgs {
    pub socket_path: Option<PathBuf>,
    pub socket_user: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SocketConfig {
    /// Path to the Unix domain socket. Defaults to `/run/openergo.sock`.
    pub path: Option<PathBuf>,
    /// User (name or UID) to own the socket at the configured path.
    pub user: Option<String>,
    /// Group (name or GID) to own the socket at the configured path. If set
    /// with `user`, overrides the user's primary group.
    pub group: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DwellClickConfig {
    /// Whether clients are allowed to configure dwell click behavior.
    pub allow: Option<bool>,
}

impl DwellClickConfig {
    pub fn allow(&self) -> bool {
        self.allow.unwrap_or(false)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevicesConfig {
    /// Whether to auto-detect keyboards, mice, and touchpads. Defaults to `true`.
    pub auto_detect: Option<bool>,
    /// Devices to include (in addition to auto-detected, or as the sole set if
    /// `auto_detect` is false). Keyed by a friendly label used in logs.
    pub include: Option<HashMap<String, DeviceMatcher>>,
    /// Devices to exclude from monitoring. Takes precedence over both
    /// auto-detected and included devices. Keyed by a friendly label used in
    /// logs.
    pub exclude: Option<HashMap<String, DeviceMatcher>>,
}

impl DevicesConfig {
    pub fn auto_detect(&self) -> bool {
        self.auto_detect.unwrap_or(true)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageConfig {
    /// Friendly device labels to ignore when computing usage. Each label must
    /// already be configured under `[devices.include]` or `[devices.exclude]`.
    pub exclude: Option<Vec<String>>,
    pub default_pointer_hand: HandConfigValue,
    pub devices: Option<HashMap<String, DeviceUsageConfig>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceUsageConfig {
    pub hand: Option<HandConfigValue>,
    pub key_profile: Option<KeyProfileConfigValue>,
    pub key_overrides: Option<HashMap<String, KeyOverrideValue>>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HandConfigValue {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeyProfileConfigValue {
    AnsiQwerty,
    None,
    AllLeft,
    AllRight,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeyOverrideValue {
    Left,
    Right,
    Unclassified,
}

/// Matches a device by path and/or udev properties. All specified fields must
/// match (AND logic). At least one field must be set.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceMatcher {
    /// Device path: matched against DEVNAME and DEVLINKS.
    pub path: Option<String>,
    /// Matched against the evdev device name (udev `NAME` property).
    pub name: Option<String>,
    /// Matched against udev `ID_MODEL`.
    pub model: Option<String>,
    /// Matched against udev `ID_MODEL_ID`.
    pub model_id: Option<String>,
    /// Matched against udev `ID_VENDOR_ID`.
    pub vendor_id: Option<String>,
    /// Matched against udev `ID_SERIAL`.
    pub serial: Option<String>,
    /// Matched against udev `ID_BUS`.
    pub bus: Option<String>,
}

impl ConfigFile {
    pub fn load(path: Option<&Path>) -> Result<Self, Report> {
        let path = load_path(path);
        let content = std::fs::read_to_string(path)
            .context("Failed to read config file")
            .attach(format!("path: {}", path.display()))?;
        let config = toml::from_str(&content).context("Failed to parse config file")?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dwell_click_allow_parses() {
        let config: ConfigFile = toml::from_str(
            r#"
            [dwell_click]
            allow = true
            "#,
        )
        .expect("config should parse");

        assert!(
            config
                .dwell_click
                .as_ref()
                .is_some_and(DwellClickConfig::allow)
        );
    }

    #[test]
    fn dwell_click_allow_defaults_to_false() {
        let config: ConfigFile = toml::from_str("").expect("empty config should parse");

        assert!(
            !config
                .dwell_click
                .as_ref()
                .is_some_and(DwellClickConfig::allow)
        );
    }

    #[test]
    fn load_uses_default_path_when_path_is_none() {
        assert_eq!(load_path(None), Path::new(DEFAULT_CONFIG_PATH));
    }

    #[test]
    fn devices_include_parses_as_map() {
        let config: ConfigFile = toml::from_str(
            r#"
            [devices.include.keyboard]
            serial = "Chicony_USB_Keyboard"

            [devices.include.mouse]
            name = "Some Mouse"
            "#,
        )
        .expect("config should parse");

        let include = config
            .devices
            .as_ref()
            .and_then(|d| d.include.as_ref())
            .expect("include should be present");
        assert_eq!(include.len(), 2);
        assert!(include.contains_key("keyboard"));
        assert!(include.contains_key("mouse"));
    }

    #[test]
    fn duplicate_label_rejected_by_toml() {
        let result: Result<ConfigFile, _> = toml::from_str(
            r#"
            [devices.include.keyboard]
            serial = "a"

            [devices.include.keyboard]
            serial = "b"
            "#,
        );
        assert!(result.is_err(), "toml must reject duplicate keys");
    }

    #[test]
    fn usage_devices_config_parses() {
        let config: ConfigFile = toml::from_str(
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

        let usage = config
            .usage
            .as_ref()
            .expect("usage config should be present");
        assert_eq!(usage.default_pointer_hand, HandConfigValue::Right);
        assert_eq!(
            usage
                .devices
                .as_ref()
                .and_then(|devices| devices.get("left_mouse"))
                .and_then(|device| device.hand),
            Some(HandConfigValue::Left)
        );
        assert_eq!(
            usage
                .devices
                .as_ref()
                .and_then(|devices| devices.get("main_keyboard"))
                .and_then(|device| device.key_overrides.as_ref())
                .and_then(|overrides| overrides.get("KEY_SPACE")),
            Some(&KeyOverrideValue::Left)
        );
        assert!(
            usage
                .devices
                .as_ref()
                .is_some_and(|devices| devices.contains_key("layer_pad"))
        );
    }

    #[test]
    fn old_usage_key_hand_config_is_rejected() {
        let result: Result<ConfigFile, _> = toml::from_str(
            r#"
            [usage]
            default_pointer_hand = "right"

            [usage.key_hand]
            profile = "ansi_qwerty"
            "#,
        );

        assert!(result.is_err(), "old usage.key_hand must be rejected");
    }

    #[test]
    fn usage_without_default_pointer_hand_is_rejected() {
        let result: Result<ConfigFile, _> = toml::from_str(
            r#"
            [usage]
            exclude = []
            "#,
        );

        assert!(
            result.is_err(),
            "usage.default_pointer_hand must be required"
        );
    }

    #[test]
    fn usage_devices_still_require_default_pointer_hand() {
        let result: Result<ConfigFile, _> = toml::from_str(
            r#"
            [devices.include.left_mouse]
            name = "Left Mouse"

            [usage.devices.left_mouse]
            hand = "left"
            "#,
        );

        assert!(
            result.is_err(),
            "per-device hands must not make default_pointer_hand optional"
        );
    }
}
