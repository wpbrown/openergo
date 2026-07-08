use crate::credit::CreditCalculatorConfig;
use crate::credit::calculator::config::{
    CreditCostConfig, CreditRateBoostConfig, GlobalCreditBoostConfig,
};
use crate::integration::{Direction, EndpointConfig};
use crate::transports::midi::MidiMessage;
use rootcause::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use tracing::info;

/// Per-endpoint catalog payload built by [`Config::build_catalog`].
/// Each variant carries a transport-specific payload that owns
/// everything the transport task needs, so `app` can move it all the
/// way through after binding without cloning.
///
/// The variant is tagged by transport, so adding HID later means adding
/// a variant here (and a matching partition arm in `app::run`'s
/// post-bind materialization). The [`integration`] crate stays
/// transport-agnostic.
///
/// [`integration`]: crate::integration
pub enum TransportConfigs {
    Midi(MidiTransportConfig),
}

impl EndpointConfig for TransportConfigs {
    fn direction(&self) -> Direction {
        match self {
            TransportConfigs::Midi(midi) => midi.control.direction,
        }
    }
}

/// Per-endpoint MIDI catalog payload. The shared device entry (its
/// key + matchers) is wrapped in [`Rc`] so every control bound on the
/// same device shares one allocation; `app::build_midi_devices`
/// reclaims it via [`Rc::try_unwrap`] after grouping. The per-control
/// entry is owned outright since each label corresponds to exactly
/// one control.
pub struct MidiTransportConfig {
    pub device: Rc<(String, MidiDeviceConfig)>,
    pub control: MidiControlConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub telemetry: Option<TelemetryConfig>,
    /// Physical devices keyed by a friendly device key. Each device
    /// contains its own `controls` map; pain and credit reference those
    /// controls by global label, never by device key.
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

impl TelemetryConfig {
    pub fn report_usage(&self) -> bool {
        self.report_usage.unwrap_or(false)
    }

    pub fn enabled(&self) -> bool {
        self.report_usage()
    }
}

/// Variant-tagged device entry. Adding HID later means adding a variant
/// here and a matching block in [`Config::validate`] /
/// [`Config::build_catalog`].
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeviceConfig {
    Midi(MidiDeviceConfig),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiDeviceConfig {
    /// Substring matched against the ALSA seq port name, as shown by
    /// `aseqdump -l`.
    pub port: Option<String>,
    /// Substring matched against the ALSA seq client name, as shown by
    /// `aseqdump -l`.
    pub client: Option<String>,
    /// Per-control map keyed by global control label.
    #[serde(default)]
    pub controls: HashMap<String, MidiControlConfig>,
}

/// Per-control MIDI binding declared in the user's config: which kind
/// of MIDI message it is, on which channel and CC/note number, and
/// which directions are valid for it. The MIDI transport sees a
/// post-binding `MidiControlDefinition` instead, with the producer /
/// consumer halves already attached.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiControlConfig {
    pub message: MidiMessage,
    /// 0..=15, matching `aseqdump`.
    pub channel: u8,
    /// CC number (when `message = "cc"`) or note number (when
    /// `message = "note"`); 0..=127.
    pub number: u8,
    #[serde(default)]
    pub direction: Direction,
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
    /// Logical pain signals, keyed by the user-facing pain label that
    /// surfaces in telemetry and persistence.
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
    /// Reference to a global control label (i.e. a `controls.<label>` key
    /// under any `[devices.*]` entry).
    pub source: String,
    /// How this signal weights toward left/right/center for downstream
    /// strain accounting.
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

/// `[credit.limits]` section. Each field is the budget for one of the
/// per-source credit accumulators. Defaults preserve the values that were
/// previously hardcoded in the CLI.
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

fn default_rest_limit() -> f64 {
    800.0
}

fn default_break_limit() -> f64 {
    2000.0
}

fn default_day_limit() -> f64 {
    30000.0
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

impl Config {
    pub fn load(path: &Path) -> Result<Self, Report> {
        let content = std::fs::read_to_string(path)
            .context("Failed to read config file")
            .attach(format!("path: {}", path.display()))?;
        let config: Config = toml::from_str(&content).context("Failed to parse config file")?;
        config.validate()?;
        info!("Loaded config from {}", path.display());
        Ok(config)
    }

    fn validate(&self) -> Result<(), Report> {
        // -- Build the global label -> resolved-MIDI-control map.
        // Walk devices in sorted order so error messages are stable.
        // Rules 1, 2, 3, 4, 5 are checked here; rule 8 is checked
        // after; rules 6 and 7 are checked while resolving references.
        let mut device_keys: Vec<&String> = self.devices.keys().collect();
        device_keys.sort();

        let mut by_label: HashMap<String, MidiControlResolution> = HashMap::new();

        for device_key in device_keys {
            let device = &self.devices[device_key];
            match device {
                DeviceConfig::Midi(midi) => {
                    // Rule 3: at least one of port / client; reject empty strings.
                    let port_match = match midi.port.as_ref() {
                        Some(s) if s.is_empty() => bail!(
                            "devices.{device_key}.port must not be empty (omit the field instead)"
                        ),
                        Some(s) => Some(s.clone()),
                        None => None,
                    };
                    let client_match = match midi.client.as_ref() {
                        Some(s) if s.is_empty() => bail!(
                            "devices.{device_key}.client must not be empty (omit the field instead)"
                        ),
                        Some(s) => Some(s.clone()),
                        None => None,
                    };
                    if port_match.is_none() && client_match.is_none() {
                        bail!(
                            "devices.{device_key}: at least one of `port` or `client` must be set"
                        );
                    }

                    // Rule 2: per-device uniqueness over (message, channel, number).
                    let mut local_keys: Vec<&String> = midi.controls.keys().collect();
                    local_keys.sort();
                    let mut seen_tuples: HashMap<(MidiMessage, u8, u8), &str> = HashMap::new();

                    for label in local_keys {
                        let control = &midi.controls[label];
                        // Rule 4: ranges.
                        if control.channel > 15 {
                            bail!(
                                "devices.{device_key}.controls.{label}.channel must be in 0..=15 (got {})",
                                control.channel,
                            );
                        }
                        if control.number > 127 {
                            bail!(
                                "devices.{device_key}.controls.{label}.number must be in 0..=127 (got {})",
                                control.number,
                            );
                        }
                        // Rule 5: note direction.
                        if control.message == MidiMessage::Note
                            && control.direction != Direction::In
                        {
                            bail!(
                                "devices.{device_key}.controls.{label}: message = \"note\" requires direction = \"in\" (got \"{}\")",
                                control.direction.as_str(),
                            );
                        }
                        // Rule 2.
                        let tuple = (control.message, control.channel, control.number);
                        if let Some(prev_label) = seen_tuples.insert(tuple, label.as_str()) {
                            bail!(
                                "devices.{device_key}: controls '{prev_label}' and '{label}' share the same (message, channel, number) tuple ({}, {}, {})",
                                control.message.as_str(),
                                control.channel,
                                control.number,
                            );
                        }

                        // Rule 1: global label uniqueness.
                        let resolution = MidiControlResolution {
                            device_key: device_key.clone(),
                            direction: control.direction,
                        };
                        if let Some(prev) = by_label.insert(label.clone(), resolution) {
                            bail!(
                                "control label '{label}' is declared twice: by devices.{} and devices.{}",
                                prev.device_key,
                                device_key,
                            );
                        }
                    }
                }
            }
        }

        // -- Validate references (rules 6, 7).
        // Pain sources reference controls; control direction must allow `in`.
        if let Some(pain) = self.pain.as_ref() {
            let mut source_names: Vec<&String> = pain.settings.sources.keys().collect();
            source_names.sort();
            for name in source_names {
                let source = &pain.settings.sources[name];
                let resolution = by_label.get(&source.source).ok_or_else(|| {
                    report!(
                        "pain.sources.{name}.source references unknown control label '{}'",
                        source.source,
                    )
                })?;
                if !resolution.direction.allows_in() {
                    bail!(
                        "pain.sources.{name}.source = '{}' has direction '{}' (must be 'in' or 'inout')",
                        source.source,
                        resolution.direction.as_str(),
                    );
                }
            }
        }

        // Credit utilization sinks reference controls; control direction must allow `out`.
        if let Some(credit) = self.credit.as_ref() {
            CreditCalculatorConfig::from_parts(
                credit.costs.clone(),
                credit.rate_boost.clone(),
                credit.global_boost.clone(),
            )
            .validate()?;

            // Rule 8: existing credit-limits checks.
            if let Some(limits) = &credit.limits {
                for (name, value) in [
                    ("rest", limits.rest),
                    ("break", limits.breaks),
                    ("day", limits.day),
                ] {
                    if !(value.is_finite() && value > 0.0) {
                        bail!("credit.limits.{name} must be > 0 (got {value})");
                    }
                }
            }

            if let Some(util) = credit.utilization.as_ref() {
                let check_sink = |field: &str, label: &str| -> Result<(), Report> {
                    let resolution = by_label.get(label).ok_or_else(|| {
                        report!(
                            "credit.utilization.{field} references unknown control label '{label}'"
                        )
                    })?;
                    if !resolution.direction.allows_out() {
                        bail!(
                            "credit.utilization.{field} = '{label}' has direction '{}' (must be 'out' or 'inout')",
                            resolution.direction.as_str(),
                        );
                    }
                    Ok(())
                };
                for (field, label) in [
                    ("rest_sink", util.rest_sink.as_ref()),
                    ("breaks_sink", util.breaks_sink.as_ref()),
                    ("day_sink", util.day_sink.as_ref()),
                ] {
                    if let Some(label) = label {
                        if label.is_empty() {
                            bail!(
                                "credit.utilization.{field} must not be empty (omit the field instead)"
                            );
                        }
                        check_sink(field, label)?;
                    }
                }
            }
        }

        Ok(())
    }
}

/// Internal scratch type used during validation to carry the per-control
/// resolved information needed for reference-checking and duplicate-label
/// diagnostics.
struct MidiControlResolution {
    device_key: String,
    direction: Direction,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_credit_config_still_parses_with_defaults() {
        let config: Config = toml::from_str(
            r#"
            [credit.limits]
            rest = 500.0
            break = 1800.0
            day = 25000.0
            "#,
        )
        .expect("config should parse");

        config.validate().expect("config should validate");
        let credit = config.credit.expect("credit config should be present");
        assert!(credit.costs.is_none());
        assert!(credit.rate_boost.is_none());
        assert!(credit.global_boost.is_none());
    }

    #[test]
    fn credit_calculator_config_parses_and_validates() {
        let config: Config = toml::from_str(
            r#"
            [credit.costs.hand]
            key = 1.5
            click = 2.0
            scroll = 0.25
            drag_per_sec = 3.0

            [credit.costs.hand.modifier]
            shift_per_sec = 5.0
            multi_per_sec = 0.5

            [credit.costs.right]
            key = 1.6

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

        config.validate().expect("config should validate");
        let credit = config.credit.expect("credit config should be present");
        let calculator_config = CreditCalculatorConfig::from_parts(
            credit.costs,
            credit.rate_boost,
            credit.global_boost,
        );
        assert_eq!(calculator_config.costs.left.key, 1.5);
        assert_eq!(calculator_config.costs.right.key, 1.6);
        assert_eq!(calculator_config.costs.unclassified.key, 1.0);
        assert_eq!(calculator_config.costs.left.modifier.shift_per_sec, 5.0);
        assert_eq!(calculator_config.costs.left.modifier.multi_per_sec, 0.5);
        assert_eq!(calculator_config.rate_boost.key.baseline_per_sec, 4.0);
    }

    #[test]
    fn invalid_credit_calculator_values_are_rejected() {
        let config: Config = toml::from_str(
            r#"
            [credit.costs.hand]
            key = -1.0
            "#,
        )
        .expect("config should parse");

        let err = config
            .validate()
            .expect_err("negative credit cost must error");
        assert!(
            format!("{err}").contains("credit.costs.left.key"),
            "unexpected error: {err}"
        );
    }

    fn parse_config(input: &str) -> Config {
        toml::from_str(input).expect("config should parse")
    }

    fn pain_check_base(indicator: &str, acknowledge: &str) -> String {
        format!(
            r#"
            [devices.grid]
            type = "midi"
            port = "grid"

            [devices.grid.controls.led_pain_stale]
            message = "cc"
            channel = 0
            number = 1
            direction = "out"

            [devices.grid.controls.btn_pain_ack]
            message = "note"
            channel = 0
            number = 2
            direction = "in"

            [pain.check]
            indicator = {indicator}
            acknowledge = {acknowledge}

            [pain.check.notifications]
            notifications = true
            sounds = true
            "#
        )
    }

    #[test]
    fn pain_check_config_parses_and_validates() {
        let config = parse_config(&pain_check_base("\"led_pain_stale\"", "\"btn_pain_ack\""));

        config.validate().expect("config should validate");
        let check = config
            .pain
            .expect("pain config should be present")
            .check
            .expect("pain check config should be present");
        assert_eq!(check.indicator.as_deref(), Some("led_pain_stale"));
        assert_eq!(check.acknowledge.as_deref(), Some("btn_pain_ack"));
        assert!(check.notifications.notifications);
        assert!(check.notifications.sounds);
    }
}
