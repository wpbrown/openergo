use super::{config, modules};
use crate::credit::calculator::config as calculator;
use crate::integration::Direction;
use crate::notifications::NotificationSettings;
use crate::pain::PainBias;
use crate::transports::midi::MidiMessage;
use rootcause::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct RuntimeConfig {
    pub server_socket_path: PathBuf,
    pub client_socket_path: PathBuf,
    pub telemetry_report_usage: Option<bool>,
    pub dwell_click_sound: bool,
    pub devices: HashMap<String, modules::endpoints::DeviceConfig>,
    pub pain: Option<modules::pain::Config>,
    pub pain_check: Option<modules::pain_check::Config>,
    pub credit: modules::credit::Config,
    pub credit_notifications: Option<NotificationSettings>,
    pub require_no_activity: bool,
    pub data_recorder: bool,
}

pub fn instantiate(
    args: config::ConfigArgs,
    file: config::ConfigFile,
) -> Result<RuntimeConfig, Report> {
    validate(&file)?;

    let config::ConfigFile {
        telemetry,
        dwell_click,
        devices,
        pain,
        credit,
        rest,
        learning,
    } = file;
    let (pain, pain_check) = match pain {
        Some(config::PainConfigGroup { settings, check }) => {
            (Some(convert_pain(settings)), check.map(convert_pain_check))
        }
        None => (None, None),
    };

    let mut credit = credit.unwrap_or_default();
    let credit_notifications = credit
        .notifications
        .take()
        .map(|cfg| NotificationSettings::new(cfg.notifications, cfg.sounds));

    Ok(RuntimeConfig {
        server_socket_path: args.server_socket_path,
        client_socket_path: args.client_socket_path,
        telemetry_report_usage: telemetry
            .and_then(|cfg| cfg.report_usage)
            .filter(|enabled| *enabled),
        dwell_click_sound: dwell_click.sound,
        devices: devices
            .into_iter()
            .map(|(key, device)| (key, convert_device(device)))
            .collect(),
        pain,
        pain_check,
        credit: convert_credit(credit)?,
        credit_notifications,
        require_no_activity: rest.unwrap_or_default().require_no_activity,
        data_recorder: learning.unwrap_or_default().data_recorder,
    })
}

fn validate(file: &config::ConfigFile) -> Result<(), Report> {
    let mut device_keys: Vec<&String> = file.devices.keys().collect();
    device_keys.sort();
    let mut by_label: HashMap<String, (&str, config::Direction)> = HashMap::new();

    for device_key in device_keys {
        match &file.devices[device_key] {
            config::DeviceConfig::Midi(midi) => {
                if midi.port.as_ref().is_some_and(String::is_empty) {
                    bail!("devices.{device_key}.port must not be empty (omit the field instead)");
                }
                if midi.client.as_ref().is_some_and(String::is_empty) {
                    bail!("devices.{device_key}.client must not be empty (omit the field instead)");
                }
                if midi.port.is_none() && midi.client.is_none() {
                    bail!("devices.{device_key}: at least one of `port` or `client` must be set");
                }

                let mut labels: Vec<&String> = midi.controls.keys().collect();
                labels.sort();
                let mut seen = HashMap::new();
                for label in labels {
                    let control = &midi.controls[label];
                    if control.channel > 15 {
                        bail!(
                            "devices.{device_key}.controls.{label}.channel must be in 0..=15 (got {})",
                            control.channel
                        );
                    }
                    if control.number > 127 {
                        bail!(
                            "devices.{device_key}.controls.{label}.number must be in 0..=127 (got {})",
                            control.number
                        );
                    }
                    if control.message == config::MidiMessage::Note
                        && control.direction != config::Direction::In
                    {
                        bail!(
                            "devices.{device_key}.controls.{label}: message = \"note\" requires direction = \"in\" (got \"{}\")",
                            control.direction.as_str()
                        );
                    }
                    let tuple = (control.message, control.channel, control.number);
                    if let Some(previous) = seen.insert(tuple, label.as_str()) {
                        bail!(
                            "devices.{device_key}: controls '{previous}' and '{label}' share the same (message, channel, number) tuple ({}, {}, {})",
                            control.message.as_str(),
                            control.channel,
                            control.number
                        );
                    }
                    if let Some((previous, _)) =
                        by_label.insert(label.clone(), (device_key.as_str(), control.direction))
                    {
                        bail!(
                            "control label '{label}' is declared twice: by devices.{previous} and devices.{device_key}"
                        );
                    }
                }
            }
        }
    }

    if let Some(pain) = &file.pain {
        let mut names: Vec<&String> = pain.settings.sources.keys().collect();
        names.sort();
        for name in names {
            let source = &pain.settings.sources[name];
            let (_, direction) = by_label.get(&source.source).ok_or_else(|| {
                report!(
                    "pain.sources.{name}.source references unknown control label '{}'",
                    source.source
                )
            })?;
            if !direction.allows_in() {
                bail!(
                    "pain.sources.{name}.source = '{}' has direction '{}' (must be 'in' or 'inout')",
                    source.source,
                    direction.as_str()
                );
            }
        }

        if let Some(check) = &pain.check {
            for (field, label, allows, expected) in [
                (
                    "indicator",
                    check.indicator.as_ref(),
                    config::Direction::allows_out as fn(config::Direction) -> bool,
                    "out or inout",
                ),
                (
                    "acknowledge",
                    check.acknowledge.as_ref(),
                    config::Direction::allows_in as fn(config::Direction) -> bool,
                    "in or inout",
                ),
            ] {
                let Some(label) = label else { continue };
                let (_, direction) = by_label.get(label).ok_or_else(|| {
                    report!("pain.check.{field} references unknown control '{label}'")
                })?;
                if !allows(*direction) {
                    bail!(
                        "pain.check.{field} = '{label}' has direction '{}' (must be {expected})",
                        direction.as_str()
                    );
                }
            }
        }
    }

    if let Some(credit) = &file.credit {
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
        if let Some(utilization) = &credit.utilization {
            for (field, label) in [
                ("rest_sink", utilization.rest_sink.as_ref()),
                ("breaks_sink", utilization.breaks_sink.as_ref()),
                ("day_sink", utilization.day_sink.as_ref()),
            ] {
                let Some(label) = label else { continue };
                if label.is_empty() {
                    bail!("credit.utilization.{field} must not be empty (omit the field instead)");
                }
                let (_, direction) = by_label.get(label).ok_or_else(|| {
                    report!("credit.utilization.{field} references unknown control label '{label}'")
                })?;
                if !direction.allows_out() {
                    bail!(
                        "credit.utilization.{field} = '{label}' has direction '{}' (must be 'out' or 'inout')",
                        direction.as_str()
                    );
                }
            }
        }
    }
    Ok(())
}

fn convert_device(device: config::DeviceConfig) -> modules::endpoints::DeviceConfig {
    match device {
        config::DeviceConfig::Midi(midi) => {
            modules::endpoints::DeviceConfig::Midi(modules::endpoints::MidiDeviceConfig {
                port: midi.port,
                client: midi.client,
                controls: midi
                    .controls
                    .into_iter()
                    .map(|(label, control)| {
                        (
                            label,
                            modules::endpoints::MidiControlConfig {
                                message: match control.message {
                                    config::MidiMessage::Cc => MidiMessage::Cc,
                                    config::MidiMessage::Note => MidiMessage::Note,
                                },
                                channel: control.channel,
                                number: control.number,
                                direction: match control.direction {
                                    config::Direction::In => Direction::In,
                                    config::Direction::Out => Direction::Out,
                                    config::Direction::InOut => Direction::InOut,
                                },
                            },
                        )
                    })
                    .collect(),
            })
        }
    }
}

fn convert_pain(raw: config::PainConfig) -> modules::pain::Config {
    let mut sources: Vec<_> = raw.sources.into_iter().collect();
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    modules::pain::Config {
        sources: sources
            .into_iter()
            .map(|(name, source)| modules::pain::SourceConfig {
                name,
                source: source.source,
                bias: match source.bias {
                    config::PainBiasConfig::Left => PainBias::Left,
                    config::PainBiasConfig::Right => PainBias::Right,
                    config::PainBiasConfig::Center => PainBias::Center,
                },
            })
            .collect(),
    }
}

fn convert_pain_check(raw: config::PainCheckConfig) -> modules::pain_check::Config {
    modules::pain_check::Config {
        indicator: raw.indicator,
        acknowledge: raw.acknowledge,
    }
}

fn convert_credit(raw: config::CreditConfig) -> Result<modules::credit::Config, Report> {
    if let Some(rate_boost) = &raw.rate_boost {
        validate_non_negative_finite("credit.rate_boost.factor", rate_boost.factor)?;
        validate_at_least_one_finite("credit.rate_boost.cap", rate_boost.cap)?;
        validate_positive_finite(
            "credit.rate_boost.smoothing_secs",
            rate_boost.smoothing_secs,
        )?;
    }
    let limits = raw.limits.unwrap_or_default();
    let calculator = calculator::CreditCalculatorConfig {
        costs: convert_costs(raw.costs.unwrap_or_default()),
        rate_boost: convert_rate_boost(raw.rate_boost.unwrap_or_default()),
        global_boost: convert_global_boost(raw.global_boost.unwrap_or_default()),
    };
    calculator.validate()?;
    Ok(modules::credit::Config {
        limits: modules::credit::LimitsConfig {
            rest: limits.rest,
            breaks: limits.breaks,
            day: limits.day,
        },
        utilization: raw
            .utilization
            .map(|cfg| modules::credit::UtilizationConfig {
                rest_sink: cfg.rest_sink,
                breaks_sink: cfg.breaks_sink,
                day_sink: cfg.day_sink,
            }),
        calculator,
    })
}

fn convert_costs(raw: config::CreditCostConfig) -> calculator::ResolvedCreditCosts {
    let mut hand = calculator::HandCostConfig::default();
    apply_hand_cost(&mut hand, raw.hand);
    let mut left = hand.clone();
    apply_hand_cost(&mut left, raw.left);
    let mut right = hand;
    apply_hand_cost(&mut right, raw.right);
    calculator::ResolvedCreditCosts {
        left,
        right,
        unclassified: calculator::UnclassifiedCostConfig {
            key: raw.unclassified.key,
            combo: raw.unclassified.combo,
        },
    }
}

fn apply_hand_cost(target: &mut calculator::HandCostConfig, raw: config::PartialHandCostConfig) {
    if let Some(value) = raw.click {
        target.click = value;
    }
    if let Some(value) = raw.drag_per_sec {
        target.drag_per_sec = value;
    }
    if let Some(value) = raw.key {
        target.key = value;
    }
    if let Some(value) = raw.scroll {
        target.scroll = value;
    }
    if let Some(value) = raw.same_hand_combo {
        target.same_hand_combo = value;
    }
    if let Some(value) = raw.modifier.shift_per_sec {
        target.modifier.shift_per_sec = value;
    }
    if let Some(value) = raw.modifier.ctrl_per_sec {
        target.modifier.ctrl_per_sec = value;
    }
    if let Some(value) = raw.modifier.alt_per_sec {
        target.modifier.alt_per_sec = value;
    }
    if let Some(value) = raw.modifier.meta_per_sec {
        target.modifier.meta_per_sec = value;
    }
    if let Some(value) = raw.modifier.multi_per_sec {
        target.modifier.multi_per_sec = value;
    }
}

fn convert_rate_boost(raw: config::CreditRateBoostConfig) -> calculator::CreditRateBoostConfig {
    let defaults = calculator::CreditRateBoostConfig::default();
    let resolve = |child: Option<config::PartialRateBoostConfig>, baseline| match child {
        Some(child) => calculator::RateBoostConfig {
            enabled: child.enabled.unwrap_or(raw.enabled),
            baseline_per_sec: child.baseline_per_sec,
            factor: child.factor.unwrap_or(raw.factor),
            cap: child.cap.unwrap_or(raw.cap),
            smoothing_secs: child.smoothing_secs.unwrap_or(raw.smoothing_secs),
        },
        None => calculator::RateBoostConfig {
            enabled: raw.enabled,
            baseline_per_sec: baseline,
            factor: raw.factor,
            cap: raw.cap,
            smoothing_secs: raw.smoothing_secs,
        },
    };
    calculator::CreditRateBoostConfig {
        key: resolve(raw.key, defaults.key.baseline_per_sec),
        click: resolve(raw.click, defaults.click.baseline_per_sec),
        scroll: resolve(raw.scroll, defaults.scroll.baseline_per_sec),
        drag: resolve(raw.drag, defaults.drag.baseline_per_sec),
        modifier: resolve(raw.modifier, defaults.modifier.baseline_per_sec),
    }
}

fn validate_non_negative_finite(field: &str, value: f64) -> Result<(), Report> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        bail!("{field} must be finite and >= 0 (got {value})")
    }
}

fn validate_positive_finite(field: &str, value: f64) -> Result<(), Report> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        bail!("{field} must be finite and > 0 (got {value})")
    }
}

fn validate_at_least_one_finite(field: &str, value: f64) -> Result<(), Report> {
    if value.is_finite() && value >= 1.0 {
        Ok(())
    } else {
        bail!("{field} must be finite and >= 1 (got {value})")
    }
}

fn convert_global_boost(
    raw: config::GlobalCreditBoostConfig,
) -> calculator::GlobalCreditBoostConfig {
    calculator::GlobalCreditBoostConfig {
        enabled: raw.enabled,
        baseline_credit_per_sec: raw.baseline_credit_per_sec,
        factor: raw.factor,
        cap: raw.cap,
        smoothing_secs: raw.smoothing_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 0.000_001,
            "expected {expected}, got {actual}"
        );
    }

    fn instantiate_toml(input: &str) -> Result<RuntimeConfig, Report> {
        instantiate(
            config::ConfigArgs {
                server_socket_path: PathBuf::from("/server.sock"),
                client_socket_path: PathBuf::from("/client.sock"),
            },
            toml::from_str(input).expect("config should parse"),
        )
    }

    #[test]
    fn credit_calculator_values_resolve_and_validate() {
        let runtime = instantiate_toml(
            r#"
            [credit.costs.hand]
            key = 1.5
            [credit.costs.right]
            key = 1.6
            [credit.rate_boost]
            enabled = true
            factor = 0.25
            cap = 1.75
            smoothing_secs = 3.0
            [credit.rate_boost.key]
            baseline_per_sec = 4.0
            "#,
        )
        .expect("config should instantiate");
        assert_eq!(runtime.credit.calculator.costs.left.key, 1.5);
        assert_eq!(runtime.credit.calculator.costs.right.key, 1.6);
    }

    #[test]
    fn credit_costs_resolve_hand_defaults_and_overrides() {
        let runtime = instantiate_toml(
            r#"
            [credit.costs.hand]
            key = 1.5
            same_hand_combo = 2.5

            [credit.costs.hand.modifier]
            shift_per_sec = 6.0

            [credit.costs.left]
            key = 1.1

            [credit.costs.left.modifier]
            ctrl_per_sec = 7.0

            [credit.costs.right]
            scroll = 0.30

            [credit.costs.unclassified]
            key = 2.0
            combo = 3.0
            "#,
        )
        .expect("config should instantiate");
        let costs = runtime.credit.calculator.costs;
        assert_close(costs.left.key, 1.1);
        assert_close(costs.right.key, 1.5);
        assert_close(costs.left.same_hand_combo, 2.5);
        assert_close(costs.right.scroll, 0.30);
        assert_close(costs.left.modifier.shift_per_sec, 6.0);
        assert_close(costs.right.modifier.shift_per_sec, 6.0);
        assert_close(costs.left.modifier.ctrl_per_sec, 7.0);
        assert_close(costs.right.modifier.ctrl_per_sec, 5.0);
        assert_close(costs.unclassified.key, 2.0);
        assert_close(costs.unclassified.combo, 3.0);
    }

    #[test]
    fn rate_boost_children_inherit_parent_and_runtime_baselines() {
        let runtime = instantiate_toml(
            r#"
            [credit.rate_boost]
            enabled = false
            factor = 0.5
            cap = 2.5
            smoothing_secs = 4.0

            [credit.rate_boost.key]
            baseline_per_sec = 6.0
            factor = 0.75

            [credit.rate_boost.modifier]
            baseline_per_sec = 0.9
            "#,
        )
        .expect("config should instantiate");
        let boost = runtime.credit.calculator.rate_boost;
        assert!(!boost.key.enabled);
        assert_close(boost.key.baseline_per_sec, 6.0);
        assert_close(boost.key.factor, 0.75);
        assert_close(boost.key.cap, 2.5);
        assert_close(boost.click.baseline_per_sec, 0.75);
        assert_close(boost.click.factor, 0.5);
        assert_close(boost.click.smoothing_secs, 4.0);
        assert_close(boost.modifier.baseline_per_sec, 0.9);
        assert_close(boost.modifier.factor, 0.5);
    }

    #[test]
    fn invalid_rate_boost_parent_is_rejected_even_when_children_override_it() {
        let error = instantiate_toml(
            r#"
            [credit.rate_boost]
            factor = -1.0

            [credit.rate_boost.key]
            baseline_per_sec = 1.0
            factor = 0.1
            [credit.rate_boost.click]
            baseline_per_sec = 1.0
            factor = 0.1
            [credit.rate_boost.scroll]
            baseline_per_sec = 1.0
            factor = 0.1
            [credit.rate_boost.drag]
            baseline_per_sec = 1.0
            factor = 0.1
            [credit.rate_boost.modifier]
            baseline_per_sec = 1.0
            factor = 0.1
            "#,
        )
        .err()
        .expect("invalid parent must error");
        assert!(format!("{error}").contains("credit.rate_boost.factor"));
    }

    #[test]
    fn pain_check_endpoint_directions_are_validated() {
        let error = instantiate_toml(
            r#"
            [devices.grid]
            type = "midi"
            port = "grid"
            [devices.grid.controls.input]
            message = "cc"
            channel = 0
            number = 1
            direction = "in"
            [pain.check]
            indicator = "input"
            "#,
        )
        .err()
        .expect("input-only indicator must error");
        assert!(format!("{error}").contains("pain.check.indicator"));
    }

    #[test]
    fn invalid_credit_calculator_values_are_rejected() {
        let error = instantiate_toml("[credit.costs.hand]\nkey = -1.0")
            .err()
            .expect("negative credit cost must error");
        assert!(format!("{error}").contains("credit.costs.left.key"));
    }

    #[test]
    fn references_and_ranges_are_validated() {
        let error = instantiate_toml(
            r#"
            [devices.grid]
            type = "midi"
            port = "grid"
            [devices.grid.controls.bad]
            message = "cc"
            channel = 16
            number = 1
            "#,
        )
        .err()
        .expect("invalid channel must error");
        assert!(format!("{error}").contains("channel must be in 0..=15"));
    }
}
