use rootcause::prelude::*;
use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone)]
pub struct CreditCalculatorConfig {
    pub costs: CreditCostConfig,
    pub rate_boost: CreditRateBoostConfig,
    pub global_boost: GlobalCreditBoostConfig,
}

impl CreditCalculatorConfig {
    pub fn from_parts(
        costs: Option<CreditCostConfig>,
        rate_boost: Option<CreditRateBoostConfig>,
        global_boost: Option<GlobalCreditBoostConfig>,
    ) -> Self {
        Self {
            costs: costs.unwrap_or_default(),
            rate_boost: rate_boost.unwrap_or_default(),
            global_boost: global_boost.unwrap_or_default(),
        }
    }

    pub fn validate(&self) -> Result<(), Report> {
        self.costs.validate()?;
        self.rate_boost.validate()?;
        self.global_boost.validate()?;
        Ok(())
    }
}

impl Default for CreditCalculatorConfig {
    fn default() -> Self {
        Self::from_parts(None, None, None)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreditCostConfig {
    #[serde(default)]
    pub key: KeyCostConfig,
    #[serde(default)]
    pub click: ClickCostConfig,
    #[serde(default)]
    pub scroll: ScrollCostConfig,
    #[serde(default)]
    pub drag: DragCostConfig,
    #[serde(default)]
    pub left_modifier: ModifierCostConfig,
    #[serde(default)]
    pub right_modifier: ModifierCostConfig,
}

impl CreditCostConfig {
    fn validate(&self) -> Result<(), Report> {
        self.key.validate("credit.costs.key")?;
        self.click.validate("credit.costs.click")?;
        self.scroll.validate("credit.costs.scroll")?;
        self.drag.validate("credit.costs.drag")?;
        self.left_modifier.validate("credit.costs.left_modifier")?;
        self.right_modifier
            .validate("credit.costs.right_modifier")?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyCostConfig {
    #[serde(default = "default_key_cost")]
    pub left: f64,
    #[serde(default = "default_key_cost")]
    pub right: f64,
    #[serde(default = "default_key_cost")]
    pub other: f64,
    #[serde(default = "default_same_hand_combo_cost")]
    pub left_combo: f64,
    #[serde(default = "default_same_hand_combo_cost")]
    pub right_combo: f64,
    #[serde(default = "default_other_hand_combo_cost")]
    pub cross_combo: f64,
    #[serde(default = "default_other_hand_combo_cost")]
    pub other_combo: f64,
}

impl KeyCostConfig {
    fn validate(&self, prefix: &str) -> Result<(), Report> {
        for (field, value) in [
            ("left", self.left),
            ("right", self.right),
            ("other", self.other),
            ("left_combo", self.left_combo),
            ("right_combo", self.right_combo),
            ("cross_combo", self.cross_combo),
            ("other_combo", self.other_combo),
        ] {
            validate_non_negative_finite(&format!("{prefix}.{field}"), value)?;
        }
        Ok(())
    }
}

impl Default for KeyCostConfig {
    fn default() -> Self {
        Self {
            left: default_key_cost(),
            right: default_key_cost(),
            other: default_key_cost(),
            left_combo: default_same_hand_combo_cost(),
            right_combo: default_same_hand_combo_cost(),
            cross_combo: default_other_hand_combo_cost(),
            other_combo: default_other_hand_combo_cost(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClickCostConfig {
    #[serde(default = "default_click_cost")]
    pub per_click: f64,
}

impl ClickCostConfig {
    fn validate(&self, prefix: &str) -> Result<(), Report> {
        validate_non_negative_finite(&format!("{prefix}.per_click"), self.per_click)
    }
}

impl Default for ClickCostConfig {
    fn default() -> Self {
        Self {
            per_click: default_click_cost(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScrollCostConfig {
    #[serde(default = "default_scroll_cost")]
    pub per_scroll: f64,
}

impl ScrollCostConfig {
    fn validate(&self, prefix: &str) -> Result<(), Report> {
        validate_non_negative_finite(&format!("{prefix}.per_scroll"), self.per_scroll)
    }
}

impl Default for ScrollCostConfig {
    fn default() -> Self {
        Self {
            per_scroll: default_scroll_cost(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DragCostConfig {
    #[serde(default = "default_drag_cost")]
    pub per_sec: f64,
}

impl DragCostConfig {
    fn validate(&self, prefix: &str) -> Result<(), Report> {
        validate_non_negative_finite(&format!("{prefix}.per_sec"), self.per_sec)
    }
}

impl Default for DragCostConfig {
    fn default() -> Self {
        Self {
            per_sec: default_drag_cost(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModifierCostConfig {
    #[serde(default = "default_shift_cost")]
    pub shift_per_sec: f64,
    #[serde(default = "default_ctrl_cost")]
    pub ctrl_per_sec: f64,
    #[serde(default = "default_alt_cost")]
    pub alt_per_sec: f64,
    #[serde(default = "default_meta_cost")]
    pub meta_per_sec: f64,
    #[serde(default = "default_multi_cost")]
    pub multi_per_sec: f64,
}

impl ModifierCostConfig {
    fn validate(&self, prefix: &str) -> Result<(), Report> {
        for (field, value) in [
            ("shift_per_sec", self.shift_per_sec),
            ("ctrl_per_sec", self.ctrl_per_sec),
            ("alt_per_sec", self.alt_per_sec),
            ("meta_per_sec", self.meta_per_sec),
            ("multi_per_sec", self.multi_per_sec),
        ] {
            validate_non_negative_finite(&format!("{prefix}.{field}"), value)?;
        }
        Ok(())
    }
}

impl Default for ModifierCostConfig {
    fn default() -> Self {
        Self {
            shift_per_sec: default_shift_cost(),
            ctrl_per_sec: default_ctrl_cost(),
            alt_per_sec: default_alt_cost(),
            meta_per_sec: default_meta_cost(),
            multi_per_sec: default_multi_cost(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreditRateBoostConfig {
    pub defaults: RateBoostDefaults,
    pub key: RateBoostConfig,
    pub click: RateBoostConfig,
    pub scroll: RateBoostConfig,
    pub drag: RateBoostConfig,
    pub left_modifier: RateBoostConfig,
    pub right_modifier: RateBoostConfig,
}

impl CreditRateBoostConfig {
    fn validate(&self) -> Result<(), Report> {
        self.defaults.validate("credit.rate_boost")?;
        for (field, config) in [
            ("credit.rate_boost.key", self.key),
            ("credit.rate_boost.click", self.click),
            ("credit.rate_boost.scroll", self.scroll),
            ("credit.rate_boost.drag", self.drag),
            ("credit.rate_boost.left_modifier", self.left_modifier),
            ("credit.rate_boost.right_modifier", self.right_modifier),
        ] {
            config.validate(field)?;
        }
        Ok(())
    }
}

impl Default for CreditRateBoostConfig {
    fn default() -> Self {
        let defaults = RateBoostDefaults::default();
        Self::from_raw(defaults, RawRateBoostChildren::default())
    }
}

impl<'de> Deserialize<'de> for CreditRateBoostConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawCreditRateBoostConfig::deserialize(deserializer)?;
        let (defaults, children) = raw.into_parts();
        Ok(Self::from_raw(defaults, children))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCreditRateBoostConfig {
    #[serde(default = "default_rate_enabled")]
    enabled: bool,
    #[serde(default = "default_rate_factor")]
    factor: f64,
    #[serde(default = "default_rate_cap")]
    cap: f64,
    #[serde(default = "default_rate_smoothing_secs")]
    smoothing_secs: f64,
    key: Option<PartialRateBoostConfig>,
    click: Option<PartialRateBoostConfig>,
    scroll: Option<PartialRateBoostConfig>,
    drag: Option<PartialRateBoostConfig>,
    left_modifier: Option<PartialRateBoostConfig>,
    right_modifier: Option<PartialRateBoostConfig>,
}

impl RawCreditRateBoostConfig {
    fn into_parts(self) -> (RateBoostDefaults, RawRateBoostChildren) {
        let defaults = RateBoostDefaults {
            enabled: self.enabled,
            factor: self.factor,
            cap: self.cap,
            smoothing_secs: self.smoothing_secs,
        };
        let children = RawRateBoostChildren {
            key: self.key,
            click: self.click,
            scroll: self.scroll,
            drag: self.drag,
            left_modifier: self.left_modifier,
            right_modifier: self.right_modifier,
        };
        (defaults, children)
    }
}

#[derive(Debug, Default)]
struct RawRateBoostChildren {
    key: Option<PartialRateBoostConfig>,
    click: Option<PartialRateBoostConfig>,
    scroll: Option<PartialRateBoostConfig>,
    drag: Option<PartialRateBoostConfig>,
    left_modifier: Option<PartialRateBoostConfig>,
    right_modifier: Option<PartialRateBoostConfig>,
}

impl CreditRateBoostConfig {
    fn from_raw(defaults: RateBoostDefaults, children: RawRateBoostChildren) -> Self {
        Self {
            defaults,
            key: RateBoostConfig::resolve(defaults, children.key, default_key_baseline_per_sec()),
            click: RateBoostConfig::resolve(
                defaults,
                children.click,
                default_click_baseline_per_sec(),
            ),
            scroll: RateBoostConfig::resolve(
                defaults,
                children.scroll,
                default_scroll_baseline_per_sec(),
            ),
            drag: RateBoostConfig::resolve(
                defaults,
                children.drag,
                default_drag_baseline_per_sec(),
            ),
            left_modifier: RateBoostConfig::resolve(
                defaults,
                children.left_modifier,
                default_modifier_baseline_per_sec(),
            ),
            right_modifier: RateBoostConfig::resolve(
                defaults,
                children.right_modifier,
                default_modifier_baseline_per_sec(),
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialRateBoostConfig {
    pub baseline_per_sec: f64,
    pub enabled: Option<bool>,
    pub factor: Option<f64>,
    pub cap: Option<f64>,
    pub smoothing_secs: Option<f64>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateBoostDefaults {
    #[serde(default = "default_rate_enabled")]
    pub enabled: bool,
    #[serde(default = "default_rate_factor")]
    pub factor: f64,
    #[serde(default = "default_rate_cap")]
    pub cap: f64,
    #[serde(default = "default_rate_smoothing_secs")]
    pub smoothing_secs: f64,
}

impl RateBoostDefaults {
    fn validate(&self, prefix: &str) -> Result<(), Report> {
        validate_non_negative_finite(&format!("{prefix}.factor"), self.factor)?;
        validate_at_least_one_finite(&format!("{prefix}.cap"), self.cap)?;
        validate_positive_finite(&format!("{prefix}.smoothing_secs"), self.smoothing_secs)?;
        Ok(())
    }
}

impl Default for RateBoostDefaults {
    fn default() -> Self {
        Self {
            enabled: default_rate_enabled(),
            factor: default_rate_factor(),
            cap: default_rate_cap(),
            smoothing_secs: default_rate_smoothing_secs(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RateBoostConfig {
    pub enabled: bool,
    pub baseline_per_sec: f64,
    pub factor: f64,
    pub cap: f64,
    pub smoothing_secs: f64,
}

impl RateBoostConfig {
    fn resolve(
        defaults: RateBoostDefaults,
        partial: Option<PartialRateBoostConfig>,
        default_baseline: f64,
    ) -> Self {
        match partial {
            Some(partial) => Self {
                enabled: partial.enabled.unwrap_or(defaults.enabled),
                baseline_per_sec: partial.baseline_per_sec,
                factor: partial.factor.unwrap_or(defaults.factor),
                cap: partial.cap.unwrap_or(defaults.cap),
                smoothing_secs: partial.smoothing_secs.unwrap_or(defaults.smoothing_secs),
            },
            None => Self {
                enabled: defaults.enabled,
                baseline_per_sec: default_baseline,
                factor: defaults.factor,
                cap: defaults.cap,
                smoothing_secs: defaults.smoothing_secs,
            },
        }
    }

    fn validate(&self, prefix: &str) -> Result<(), Report> {
        validate_positive_finite(&format!("{prefix}.baseline_per_sec"), self.baseline_per_sec)?;
        validate_non_negative_finite(&format!("{prefix}.factor"), self.factor)?;
        validate_at_least_one_finite(&format!("{prefix}.cap"), self.cap)?;
        validate_positive_finite(&format!("{prefix}.smoothing_secs"), self.smoothing_secs)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
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

impl GlobalCreditBoostConfig {
    fn validate(&self) -> Result<(), Report> {
        validate_positive_finite(
            "credit.global_boost.baseline_credit_per_sec",
            self.baseline_credit_per_sec,
        )?;
        validate_non_negative_finite("credit.global_boost.factor", self.factor)?;
        validate_at_least_one_finite("credit.global_boost.cap", self.cap)?;
        validate_positive_finite("credit.global_boost.smoothing_secs", self.smoothing_secs)?;
        Ok(())
    }
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

fn default_key_cost() -> f64 {
    1.0
}

fn default_same_hand_combo_cost() -> f64 {
    1.25
}

fn default_other_hand_combo_cost() -> f64 {
    1.10
}

fn default_click_cost() -> f64 {
    2.0
}

fn default_scroll_cost() -> f64 {
    0.25
}

fn default_drag_cost() -> f64 {
    3.0
}

fn default_shift_cost() -> f64 {
    5.0
}

fn default_ctrl_cost() -> f64 {
    5.0
}

fn default_alt_cost() -> f64 {
    3.0
}

fn default_meta_cost() -> f64 {
    3.0
}

fn default_multi_cost() -> f64 {
    1.0
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

fn default_key_baseline_per_sec() -> f64 {
    4.0
}

fn default_click_baseline_per_sec() -> f64 {
    0.75
}

fn default_scroll_baseline_per_sec() -> f64 {
    8.0
}

fn default_drag_baseline_per_sec() -> f64 {
    0.20
}

fn default_modifier_baseline_per_sec() -> f64 {
    0.30
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

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 0.000_001,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn rate_boost_child_inherits_parent_defaults() {
        let config: CreditRateBoostConfig = toml::from_str(
            r#"
            enabled = false
            factor = 0.5
            cap = 2.5
            smoothing_secs = 4.0

            [key]
            baseline_per_sec = 6.0
            factor = 0.75
            "#,
        )
        .expect("rate boost config should parse");

        assert!(!config.key.enabled);
        assert_close(config.key.baseline_per_sec, 6.0);
        assert_close(config.key.factor, 0.75);
        assert_close(config.key.cap, 2.5);
        assert_close(config.click.baseline_per_sec, 0.75);
        assert_close(config.click.factor, 0.5);
        assert_close(config.click.smoothing_secs, 4.0);
    }

    #[test]
    fn present_rate_child_requires_baseline() {
        let result: Result<CreditRateBoostConfig, _> = toml::from_str(
            r#"
            [key]
            factor = 0.75
            "#,
        );

        assert!(result.is_err(), "present child table must require baseline");
    }
}
