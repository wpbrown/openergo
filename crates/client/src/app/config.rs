use directories::ProjectDirs;
use rootcause::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::info;

/// Command-line configuration values that override the TOML file.
pub struct ConfigArgs {
    pub server_socket_path: PathBuf,
    pub client_socket_path: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub telemetry: Option<TelemetryConfig>,
    #[serde(default)]
    pub devices: HashMap<String, DeviceConfig>,
    pub pain: Option<PainConfigGroup>,
    pub credit: Option<CreditConfig>,
    pub rest: Option<RestConfig>,
    pub learning: Option<LearningConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningConfig {
    #[serde(default)]
    pub data_recorder: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestConfig {
    #[serde(default)]
    pub require_no_activity: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryConfig {
    pub report_usage: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeviceConfig {
    Midi(MidiDeviceConfig),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiDeviceConfig {
    pub port: Option<String>,
    pub client: Option<String>,
    #[serde(default)]
    pub controls: HashMap<String, MidiControlConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiControlConfig {
    pub message: MidiMessage,
    pub channel: u8,
    pub number: u8,
    #[serde(default)]
    pub direction: Direction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiMessage {
    Cc,
    Note,
}

impl MidiMessage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cc => "cc",
            Self::Note => "note",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    In,
    Out,
    InOut,
}

impl Direction {
    pub fn allows_in(self) -> bool {
        matches!(self, Self::In | Self::InOut)
    }

    pub fn allows_out(self) -> bool {
        matches!(self, Self::Out | Self::InOut)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
            Self::InOut => "inout",
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PainConfigGroup {
    #[serde(flatten)]
    pub settings: PainConfig,
    pub check: Option<PainCheckConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PainConfig {
    #[serde(default)]
    pub sources: HashMap<String, PainSourceConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PainCheckConfig {
    pub indicator: Option<String>,
    pub acknowledge: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub notifications: PainCheckNotificationsConfig,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct PainCheckNotificationsConfig {
    #[serde(default)]
    pub notifications: bool,
    #[serde(default)]
    pub sounds: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PainSourceConfig {
    pub source: String,
    pub bias: PainBiasConfig,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PainBiasConfig {
    Left,
    Right,
    Center,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreditConfig {
    pub limits: Option<CreditLimitsConfig>,
    pub utilization: Option<CreditUtilizationConfig>,
    pub notifications: Option<CreditNotificationsConfig>,
    pub costs: Option<CreditCostConfig>,
    pub rate_boost: Option<CreditRateBoostConfig>,
    pub global_boost: Option<GlobalCreditBoostConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreditUtilizationConfig {
    pub rest_sink: Option<String>,
    pub breaks_sink: Option<String>,
    pub day_sink: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreditNotificationsConfig {
    #[serde(default)]
    pub notifications: bool,
    #[serde(default)]
    pub sounds: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreditLimitsConfig {
    #[serde(default = "default_rest_limit")]
    pub rest: f64,
    #[serde(default = "default_break_limit", rename = "break")]
    pub breaks: f64,
    #[serde(default = "default_day_limit")]
    pub day: f64,
}

impl Default for CreditLimitsConfig {
    fn default() -> Self {
        Self {
            rest: default_rest_limit(),
            breaks: default_break_limit(),
            day: default_day_limit(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreditCostConfig {
    #[serde(default)]
    pub hand: PartialHandCostConfig,
    #[serde(default)]
    pub left: PartialHandCostConfig,
    #[serde(default)]
    pub right: PartialHandCostConfig,
    #[serde(default)]
    pub unclassified: UnclassifiedCostConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialHandCostConfig {
    pub click: Option<f64>,
    pub drag_per_sec: Option<f64>,
    pub key: Option<f64>,
    pub scroll: Option<f64>,
    pub same_hand_combo: Option<f64>,
    #[serde(default)]
    pub modifier: PartialModifierCostConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialModifierCostConfig {
    pub shift_per_sec: Option<f64>,
    pub ctrl_per_sec: Option<f64>,
    pub alt_per_sec: Option<f64>,
    pub meta_per_sec: Option<f64>,
    pub multi_per_sec: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnclassifiedCostConfig {
    #[serde(default = "default_key_cost")]
    pub key: f64,
    #[serde(default = "default_unclassified_combo_cost")]
    pub combo: f64,
}

impl Default for UnclassifiedCostConfig {
    fn default() -> Self {
        Self {
            key: default_key_cost(),
            combo: default_unclassified_combo_cost(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreditRateBoostConfig {
    #[serde(default = "default_rate_enabled")]
    pub enabled: bool,
    #[serde(default = "default_rate_factor")]
    pub factor: f64,
    #[serde(default = "default_rate_cap")]
    pub cap: f64,
    #[serde(default = "default_rate_smoothing_secs")]
    pub smoothing_secs: f64,
    pub key: Option<PartialRateBoostConfig>,
    pub click: Option<PartialRateBoostConfig>,
    pub scroll: Option<PartialRateBoostConfig>,
    pub drag: Option<PartialRateBoostConfig>,
    pub modifier: Option<PartialRateBoostConfig>,
}

impl Default for CreditRateBoostConfig {
    fn default() -> Self {
        Self {
            enabled: default_rate_enabled(),
            factor: default_rate_factor(),
            cap: default_rate_cap(),
            smoothing_secs: default_rate_smoothing_secs(),
            key: None,
            click: None,
            scroll: None,
            drag: None,
            modifier: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialRateBoostConfig {
    pub baseline_per_sec: f64,
    pub enabled: Option<bool>,
    pub factor: Option<f64>,
    pub cap: Option<f64>,
    pub smoothing_secs: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalCreditBoostConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_global_baseline_credit_per_sec")]
    pub baseline_credit_per_sec: f64,
    #[serde(default = "default_global_factor")]
    pub factor: f64,
    #[serde(default = "default_global_cap")]
    pub cap: f64,
    #[serde(default = "default_global_smoothing_secs")]
    pub smoothing_secs: f64,
}

impl Default for GlobalCreditBoostConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            baseline_credit_per_sec: default_global_baseline_credit_per_sec(),
            factor: default_global_factor(),
            cap: default_global_cap(),
            smoothing_secs: default_global_smoothing_secs(),
        }
    }
}

impl ConfigFile {
    pub fn load(path: Option<&Path>) -> Result<Self, Report> {
        let (path, explicit) = match path {
            Some(path) => (path.to_path_buf(), true),
            None => (default_path(), false),
        };
        if !path.exists() {
            if explicit {
                bail!("specified config file not found at {}", path.display());
            }
            info!("no config file found at {}, using defaults", path.display());
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .context("Failed to read config file")
            .attach(format!("path: {}", path.display()))?;
        let config = toml::from_str(&content).context("Failed to parse config file")?;
        info!("Parsed config from {}", path.display());
        Ok(config)
    }
}

fn default_path() -> PathBuf {
    ProjectDirs::from("", "", "openergo")
        .map(|dirs| dirs.config_dir().join("client.toml"))
        .unwrap_or_else(|| PathBuf::from("client.toml"))
}

fn default_rest_limit() -> f64 {
    800.0
}
fn default_break_limit() -> f64 {
    2000.0
}
fn default_day_limit() -> f64 {
    30000.0
}
fn default_key_cost() -> f64 {
    1.0
}
fn default_unclassified_combo_cost() -> f64 {
    1.10
}
fn default_rate_enabled() -> bool {
    true
}
fn default_rate_factor() -> f64 {
    0.25
}
fn default_rate_cap() -> f64 {
    1.75
}
fn default_rate_smoothing_secs() -> f64 {
    3.0
}
fn default_global_baseline_credit_per_sec() -> f64 {
    8.0
}
fn default_global_factor() -> f64 {
    0.20
}
fn default_global_cap() -> f64 {
    1.5
}
fn default_global_smoothing_secs() -> f64 {
    10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_credit_config_parses_with_defaults() {
        let config: ConfigFile = toml::from_str(
            r#"
            [credit.limits]
            rest = 500.0
            break = 1800.0
            day = 25000.0
            "#,
        )
        .expect("config should parse");
        let credit = config.credit.expect("credit config should be present");
        assert!(credit.costs.is_none());
        assert!(credit.rate_boost.is_none());
        assert!(credit.global_boost.is_none());
    }

    #[test]
    fn credit_calculator_shapes_parse() {
        let config: ConfigFile = toml::from_str(
            r#"
            [credit.costs.hand]
            key = 1.5
            [credit.costs.hand.modifier]
            shift_per_sec = 5.0
            [credit.rate_boost]
            enabled = true
            factor = 0.25
            cap = 1.75
            smoothing_secs = 3.0
            [credit.rate_boost.key]
            baseline_per_sec = 4.0
            [credit.global_boost]
            enabled = false
            "#,
        )
        .expect("config should parse");
        let credit = config.credit.expect("credit config should be present");
        assert_eq!(credit.costs.expect("costs").hand.key, Some(1.5));
        assert_eq!(
            credit
                .rate_boost
                .expect("rate boost")
                .key
                .expect("key")
                .baseline_per_sec,
            4.0
        );
    }

    #[test]
    fn old_credit_cost_sections_are_rejected() {
        let config: Result<ConfigFile, _> = toml::from_str("[credit.costs.key]\nleft = 1.0");
        assert!(config.is_err());
    }

    #[test]
    fn present_rate_child_requires_baseline() {
        let config: Result<ConfigFile, _> =
            toml::from_str("[credit.rate_boost.key]\nfactor = 0.75");
        assert!(config.is_err());
    }

    #[test]
    fn pain_check_shape_parses() {
        let config: ConfigFile = toml::from_str(
            r#"
            [pain.check]
            indicator = "led"
            acknowledge = "button"
            [pain.check.notifications]
            notifications = true
            sounds = true
            "#,
        )
        .expect("config should parse");
        let check = config.pain.expect("pain").check.expect("check");
        assert_eq!(check.indicator.as_deref(), Some("led"));
        assert!(check.notifications.notifications);
        assert!(check.notifications.sounds);
    }
}
