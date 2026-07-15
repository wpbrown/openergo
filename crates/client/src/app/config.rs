use directories::ProjectDirs;
use rootcause::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::info;

/// Command-line configuration values that override the TOML file.
pub struct ConfigArgs {
    pub server_socket_path: PathBuf,
    pub client_socket_path: PathBuf,
}

/// Client TOML configuration file.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    /// OpenTelemetry usage reporting settings.
    pub telemetry: Option<TelemetryConfig>,
    /// External input and output devices, keyed by a unique device name.
    #[serde(default)]
    pub devices: HashMap<String, DeviceConfig>,
    /// Pain reporting sources and pain-check settings.
    pub pain: Option<PainConfigGroup>,
    /// Usage credit limits, costs, boosts, and notifications.
    pub credit: Option<CreditConfig>,
    /// Rest detection settings.
    pub rest: Option<RestConfig>,
    /// Data collection settings for future learning features.
    pub learning: Option<LearningConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LearningConfig {
    /// Whether to record labeled usage data for future model training. Defaults
    /// to `false`.
    #[serde(default)]
    pub data_recorder: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestConfig {
    /// Whether rest credit is earned only while there is no user activity.
    /// Defaults to `false`.
    #[serde(default)]
    pub require_no_activity: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TelemetryConfig {
    /// Whether to report usage as OpenTelemetry metrics. Defaults to `false`.
    pub report_usage: Option<bool>,
}

/// An external device used as a source or sink for client integrations.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(title = "Device")]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeviceConfig {
    /// A MIDI device selected by port, client, or both.
    Midi(MidiDeviceConfig),
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MidiDeviceConfig {
    /// MIDI port name to match. At least one of `port` and `client` must be set.
    pub port: Option<String>,
    /// MIDI client name to match. At least one of `port` and `client` must be
    /// set.
    pub client: Option<String>,
    /// MIDI controls exposed by this device, keyed by a globally unique control
    /// label.
    #[serde(default)]
    pub controls: HashMap<String, MidiControlConfig>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(title = "MidiControl")]
#[serde(deny_unknown_fields)]
pub struct MidiControlConfig {
    /// MIDI message kind handled by this control.
    pub message: MidiMessage,
    /// Zero-based MIDI channel in the range 0 through 15.
    pub channel: u8,
    /// MIDI controller or note number in the range 0 through 127.
    pub number: u8,
    /// Whether the control is an input, output, or both. Defaults to `"in"`.
    #[serde(default)]
    pub direction: Direction,
}

/// MIDI message kind used by a control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, JsonSchema)]
#[schemars(title = "MidiMessage")]
#[serde(rename_all = "snake_case")]
pub enum MidiMessage {
    /// MIDI control change message.
    Cc,
    /// MIDI note message. Note controls support input only.
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

/// Direction in which an integration control may be used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[schemars(title = "Direction")]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Input from the external device into Openergo.
    #[default]
    In,
    /// Output from Openergo to the external device.
    Out,
    /// Both input and output.
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

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PainConfigGroup {
    /// Pain-reporting sources. This is flattened so `sources` appears directly
    /// under `[pain]`.
    #[serde(flatten)]
    pub settings: PainConfig,
    /// Interactive pain-check controls and notifications.
    pub check: Option<PainCheckConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PainConfig {
    /// Pain-reporting inputs, keyed by a unique pain source label.
    #[serde(default)]
    pub sources: HashMap<String, PainSourceConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PainCheckConfig {
    /// Output control used to indicate that a pain check is active.
    pub indicator: Option<String>,
    /// Input control used to acknowledge a pain check.
    pub acknowledge: Option<String>,
    /// Desktop notification and sound settings for pain checks.
    #[serde(default)]
    #[allow(dead_code)]
    pub notifications: PainCheckNotificationsConfig,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct PainCheckNotificationsConfig {
    /// Whether to show desktop notifications. Defaults to `false`.
    #[serde(default)]
    pub notifications: bool,
    /// Whether to play notification sounds. Defaults to `false`.
    #[serde(default)]
    pub sounds: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(title = "PainSource")]
#[serde(deny_unknown_fields)]
pub struct PainSourceConfig {
    /// Global control label supplying pain values. The control must allow input.
    pub source: String,
    /// Body-side bias associated with values from this source.
    pub bias: PainBiasConfig,
}

/// Body-side bias associated with a pain source.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[schemars(title = "Bias")]
#[serde(rename_all = "snake_case")]
pub enum PainBiasConfig {
    /// Left side of the body.
    Left,
    /// Right side of the body.
    Right,
    /// No left or right bias.
    Center,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreditConfig {
    /// Credit thresholds for rest breaks, longer breaks, and daily usage.
    pub limits: Option<CreditLimitsConfig>,
    /// Output controls that display current credit utilization.
    pub utilization: Option<CreditUtilizationConfig>,
    /// Credit-related desktop notification and sound settings.
    pub notifications: Option<CreditNotificationsConfig>,
    /// Base credit costs for classified usage events.
    pub costs: Option<CreditCostConfig>,
    /// Per-activity multipliers for sustained high activity rates.
    pub rate_boost: Option<CreditRateBoostConfig>,
    /// Multiplier for a sustained high total credit-consumption rate.
    pub global_boost: Option<GlobalCreditBoostConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreditUtilizationConfig {
    /// Output control that receives rest-credit utilization.
    pub rest_sink: Option<String>,
    /// Output control that receives break-credit utilization.
    pub breaks_sink: Option<String>,
    /// Output control that receives daily-credit utilization.
    pub day_sink: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreditNotificationsConfig {
    /// Whether to show desktop notifications for credit events. Defaults to
    /// `false`.
    #[serde(default)]
    pub notifications: bool,
    /// Whether to play sounds for credit events. Defaults to `false`.
    #[serde(default)]
    pub sounds: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreditLimitsConfig {
    /// Credit limit before suggesting a micro-rest. Defaults to 800.
    #[serde(default = "default_rest_limit")]
    pub rest: f64,
    /// Credit limit before suggesting a longer break. Defaults to 2000.
    #[serde(default = "default_break_limit", rename = "break")]
    pub breaks: f64,
    /// Daily credit limit. Defaults to 30000.
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

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreditCostConfig {
    /// Base costs shared by both hands.
    #[serde(default)]
    pub hand: PartialHandCostConfig,
    /// Overrides to base costs for left-handed usage.
    #[serde(default)]
    pub left: PartialHandCostConfig,
    /// Overrides to base costs for right-handed usage.
    #[serde(default)]
    pub right: PartialHandCostConfig,
    /// Costs for usage that cannot be assigned to either hand.
    #[serde(default)]
    pub unclassified: UnclassifiedCostConfig,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[schemars(title = "HandCosts")]
#[serde(deny_unknown_fields)]
pub struct PartialHandCostConfig {
    /// Credit cost per click.
    pub click: Option<f64>,
    /// Credit cost per second of dragging.
    pub drag_per_sec: Option<f64>,
    /// Credit cost per key press.
    pub key: Option<f64>,
    /// Credit cost per scroll unit.
    pub scroll: Option<f64>,
    /// Multiplier applied to same-hand key combinations.
    pub same_hand_combo: Option<f64>,
    /// Credit costs per second of holding modifier keys.
    #[serde(default)]
    pub modifier: PartialModifierCostConfig,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[schemars(title = "ModifierCosts")]
#[serde(deny_unknown_fields)]
pub struct PartialModifierCostConfig {
    /// Credit cost per second of holding Shift.
    pub shift_per_sec: Option<f64>,
    /// Credit cost per second of holding Control.
    pub ctrl_per_sec: Option<f64>,
    /// Credit cost per second of holding Alt.
    pub alt_per_sec: Option<f64>,
    /// Credit cost per second of holding Meta.
    pub meta_per_sec: Option<f64>,
    /// Credit cost per second when multiple modifiers are held.
    pub multi_per_sec: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnclassifiedCostConfig {
    /// Credit cost per unclassified key press. Defaults to 1.
    #[serde(default = "default_key_cost")]
    pub key: f64,
    /// Multiplier applied to unclassified key combinations. Defaults to 1.10.
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

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreditRateBoostConfig {
    /// Whether per-activity rate boosts are enabled. Defaults to `true`.
    #[serde(default = "default_rate_enabled")]
    pub enabled: bool,
    /// Default boost added per multiple of the baseline rate. Defaults to 0.25.
    #[serde(default = "default_rate_factor")]
    pub factor: f64,
    /// Default maximum activity cost multiplier. Defaults to 1.75.
    #[serde(default = "default_rate_cap")]
    pub cap: f64,
    /// Default smoothing window in seconds. Defaults to 3.
    #[serde(default = "default_rate_smoothing_secs")]
    pub smoothing_secs: f64,
    /// Key-press rate boost settings.
    pub key: Option<PartialRateBoostConfig>,
    /// Click rate boost settings.
    pub click: Option<PartialRateBoostConfig>,
    /// Scroll rate boost settings.
    pub scroll: Option<PartialRateBoostConfig>,
    /// Drag rate boost settings.
    pub drag: Option<PartialRateBoostConfig>,
    /// Modifier-hold rate boost settings.
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

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(title = "RateBoost")]
#[serde(deny_unknown_fields)]
pub struct PartialRateBoostConfig {
    /// Activity rate per second above which boosting begins. Required whenever
    /// an activity-specific table is present.
    pub baseline_per_sec: f64,
    /// Whether this activity's boost is enabled. Inherits the parent setting
    /// when omitted.
    pub enabled: Option<bool>,
    /// Boost added per multiple of the baseline. Inherits the parent setting
    /// when omitted.
    pub factor: Option<f64>,
    /// Maximum activity cost multiplier. Inherits the parent setting when
    /// omitted.
    pub cap: Option<f64>,
    /// Smoothing window in seconds. Inherits the parent setting when omitted.
    pub smoothing_secs: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GlobalCreditBoostConfig {
    /// Whether the global credit-rate boost is enabled. Defaults to `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Total credit consumption rate per second above which boosting begins.
    /// Defaults to 8.
    #[serde(default = "default_global_baseline_credit_per_sec")]
    pub baseline_credit_per_sec: f64,
    /// Boost added per multiple of the baseline rate. Defaults to 0.20.
    #[serde(default = "default_global_factor")]
    pub factor: f64,
    /// Maximum global cost multiplier. Defaults to 1.5.
    #[serde(default = "default_global_cap")]
    pub cap: f64,
    /// Smoothing window in seconds. Defaults to 10.
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
