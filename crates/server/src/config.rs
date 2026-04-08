use rootcause::prelude::*;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub socket: Option<SocketConfig>,
    pub dwell_click: Option<DwellClickConfig>,
    pub devices: Option<DevicesConfig>,
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
    /// `auto_detect` is false).
    pub include: Option<Vec<DeviceMatcher>>,
    /// Devices to exclude from monitoring. Takes precedence over both
    /// auto-detected and included devices.
    pub exclude: Option<Vec<DeviceMatcher>>,
}

impl DevicesConfig {
    pub fn auto_detect(&self) -> bool {
        self.auto_detect.unwrap_or(true)
    }
}

/// Matches a device by path and/or udev properties. All specified fields must
/// match (AND logic). At least one field must be set.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceMatcher {
    /// Device path — matched against DEVNAME and DEVLINKS.
    pub path: Option<String>,
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
            && self.model.is_none()
            && self.model_id.is_none()
            && self.vendor_id.is_none()
            && self.serial.is_none()
            && self.bus.is_none()
    }
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
        let Some(devices) = &self.devices else {
            return Ok(());
        };

        // Validate individual matchers have at least one field.
        let validate_matchers = |matchers: &[DeviceMatcher], label: &str| -> Result<(), Report> {
            for (i, m) in matchers.iter().enumerate() {
                if m.is_empty() {
                    bail!("devices.{label}[{i}] has no fields set");
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

        if !devices.auto_detect() && devices.include.as_ref().is_none_or(|v| v.is_empty()) {
            bail!(
                "auto_detect is false and no include rules are set; \
                 no devices would be monitored"
            );
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
}
