use rootcause::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub socket: Option<SocketConfig>,
    pub dwell_click: Option<DwellClickConfig>,
    pub devices: Option<DevicesConfig>,
    pub usage: Option<UsageConfig>,
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

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageConfig {
    /// Friendly device labels to ignore when computing usage. Each label must
    /// already be defined under `[devices.include]`.
    pub exclude: Option<Vec<String>>,
}

/// Matches a device by path and/or udev properties. All specified fields must
/// match (AND logic). At least one field must be set.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceMatcher {
    /// Device path — matched against DEVNAME and DEVLINKS.
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

impl DeviceMatcher {
    fn is_empty(&self) -> bool {
        self.path.is_none()
            && self.name.is_none()
            && self.model.is_none()
            && self.model_id.is_none()
            && self.vendor_id.is_none()
            && self.serial.is_none()
            && self.bus.is_none()
    }
}

/// Returns true if `label` is a valid friendly device label: non-empty and
/// composed only of ASCII alphanumerics, `_`, or `-`.
fn is_valid_label(label: &str) -> bool {
    !label.is_empty()
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, Report> {
        let content = std::fs::read_to_string(path)
            .context("Failed to read config file")
            .attach(format!("path: {}", path.display()))?;
        let config: Config = toml::from_str(&content).context("Failed to parse config file")?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), Report> {
        if let Some(devices) = &self.devices {
            let validate_matchers =
                |matchers: &HashMap<String, DeviceMatcher>, section: &str| -> Result<(), Report> {
                    for (label, matcher) in matchers {
                        if !is_valid_label(label) {
                            bail!(
                                "devices.{section} key {label:?} is not a valid label \
                                 (must be non-empty ASCII alphanumerics, '_' or '-')"
                            );
                        }
                        if matcher.is_empty() {
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

        if let Some(usage) = &self.usage
            && let Some(exclude) = &usage.exclude
        {
            for label in exclude {
                if !is_valid_label(label) {
                    bail!(
                        "usage.exclude entry {label:?} is not a valid label \
                         (must be non-empty ASCII alphanumerics, '_' or '-')"
                    );
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dwell_click_allow_parses() {
        let config: Config = toml::from_str(
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
        let config: Config = toml::from_str("").expect("empty config should parse");

        assert!(
            !config
                .dwell_click
                .as_ref()
                .is_some_and(DwellClickConfig::allow)
        );
    }

    #[test]
    fn devices_include_parses_as_map() {
        let config: Config = toml::from_str(
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
        config.validate().expect("config should validate");
    }

    #[test]
    fn empty_matcher_rejected() {
        let config: Config = toml::from_str(
            r#"
            [devices.include.keyboard]
            "#,
        )
        .expect("config should parse");

        let err = config.validate().expect_err("empty matcher must error");
        assert!(
            format!("{err}").contains("devices.include.keyboard"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn invalid_label_rejected() {
        let config: Config = toml::from_str(
            r#"
            [devices.include."bad label"]
            serial = "x"
            "#,
        )
        .expect("config should parse");

        let err = config.validate().expect_err("invalid label must error");
        assert!(
            format!("{err}").contains("not a valid label"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn empty_label_rejected() {
        let config: Config = toml::from_str(
            r#"
            [devices.include.""]
            serial = "x"
            "#,
        )
        .expect("config should parse");

        let err = config.validate().expect_err("empty label must error");
        assert!(
            format!("{err}").contains("not a valid label"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn auto_detect_off_with_no_includes_rejected() {
        let config: Config = toml::from_str(
            r#"
            [devices]
            auto_detect = false
            "#,
        )
        .expect("config should parse");

        let err = config
            .validate()
            .expect_err("must reject when nothing would be monitored");
        assert!(
            format!("{err}").contains("auto_detect is false"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn duplicate_label_rejected_by_toml() {
        let result: Result<Config, _> = toml::from_str(
            r#"
            [devices.include.keyboard]
            serial = "a"

            [devices.include.keyboard]
            serial = "b"
            "#,
        );
        assert!(result.is_err(), "toml must reject duplicate keys");
    }
}
