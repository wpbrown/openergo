use rootcause::prelude::*;
use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone)]
pub struct CreditCalculatorConfig {
    pub costs: ResolvedCreditCosts,
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
            costs: costs.unwrap_or_default().resolve(),
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
    pub hand: PartialHandCostConfig,
    #[serde(default)]
    pub left: PartialHandCostConfig,
    #[serde(default)]
    pub right: PartialHandCostConfig,
    #[serde(default)]
    pub unclassified: UnclassifiedCostConfig,
}

impl CreditCostConfig {
    fn resolve(&self) -> ResolvedCreditCosts {
        let mut hand = HandCostConfig::default();
        self.hand.apply_to(&mut hand);

        let mut left = hand.clone();
        self.left.apply_to(&mut left);

        let mut right = hand;
        self.right.apply_to(&mut right);

        ResolvedCreditCosts {
            left,
            right,
            unclassified: self.unclassified.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedCreditCosts {
    pub left: HandCostConfig,
    pub right: HandCostConfig,
    pub unclassified: UnclassifiedCostConfig,
}

impl ResolvedCreditCosts {
    fn validate(&self) -> Result<(), Report> {
        self.left.validate("credit.costs.left")?;
        self.right.validate("credit.costs.right")?;
        self.unclassified.validate("credit.costs.unclassified")?;
        Ok(())
    }
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

impl PartialHandCostConfig {
    fn apply_to(&self, costs: &mut HandCostConfig) {
        if let Some(click) = self.click {
            costs.click = click;
        }
        if let Some(drag_per_sec) = self.drag_per_sec {
            costs.drag_per_sec = drag_per_sec;
        }
        if let Some(key) = self.key {
            costs.key = key;
        }
        if let Some(scroll) = self.scroll {
            costs.scroll = scroll;
        }
        if let Some(same_hand_combo) = self.same_hand_combo {
            costs.same_hand_combo = same_hand_combo;
        }
        self.modifier.apply_to(&mut costs.modifier);
    }
}

#[derive(Debug, Clone)]
pub struct HandCostConfig {
    pub click: f64,
    pub drag_per_sec: f64,
    pub key: f64,
    pub scroll: f64,
    pub same_hand_combo: f64,
    pub modifier: ModifierCostConfig,
}

impl HandCostConfig {
    fn validate(&self, prefix: &str) -> Result<(), Report> {
        for (field, value) in [
            ("click", self.click),
            ("drag_per_sec", self.drag_per_sec),
            ("key", self.key),
            ("scroll", self.scroll),
            ("same_hand_combo", self.same_hand_combo),
        ] {
            validate_non_negative_finite(&format!("{prefix}.{field}"), value)?;
        }
        self.modifier.validate(&format!("{prefix}.modifier"))?;
        Ok(())
    }
}

impl Default for HandCostConfig {
    fn default() -> Self {
        Self {
            click: default_click_cost(),
            drag_per_sec: default_drag_cost(),
            key: default_key_cost(),
            scroll: default_scroll_cost(),
            same_hand_combo: default_same_hand_combo_cost(),
            modifier: ModifierCostConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnclassifiedCostConfig {
    #[serde(default = "default_key_cost")]
    pub key: f64,
    #[serde(default = "default_unclassified_combo_cost")]
    pub combo: f64,
}

impl UnclassifiedCostConfig {
    fn validate(&self, prefix: &str) -> Result<(), Report> {
        validate_non_negative_finite(&format!("{prefix}.key"), self.key)?;
        validate_non_negative_finite(&format!("{prefix}.combo"), self.combo)?;
        Ok(())
    }
}

impl Default for UnclassifiedCostConfig {
    fn default() -> Self {
        Self {
            key: default_key_cost(),
            combo: default_unclassified_combo_cost(),
        }
    }
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

impl PartialModifierCostConfig {
    fn apply_to(&self, costs: &mut ModifierCostConfig) {
        if let Some(shift_per_sec) = self.shift_per_sec {
            costs.shift_per_sec = shift_per_sec;
        }
        if let Some(ctrl_per_sec) = self.ctrl_per_sec {
            costs.ctrl_per_sec = ctrl_per_sec;
        }
        if let Some(alt_per_sec) = self.alt_per_sec {
            costs.alt_per_sec = alt_per_sec;
        }
        if let Some(meta_per_sec) = self.meta_per_sec {
            costs.meta_per_sec = meta_per_sec;
        }
        if let Some(multi_per_sec) = self.multi_per_sec {
            costs.multi_per_sec = multi_per_sec;
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
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
    pub modifier: RateBoostConfig,
}

impl CreditRateBoostConfig {
    fn validate(&self) -> Result<(), Report> {
        self.defaults.validate("credit.rate_boost")?;
        for (field, config) in [
            ("credit.rate_boost.key", &self.key),
            ("credit.rate_boost.click", &self.click),
            ("credit.rate_boost.scroll", &self.scroll),
            ("credit.rate_boost.drag", &self.drag),
            ("credit.rate_boost.modifier", &self.modifier),
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
    modifier: Option<PartialRateBoostConfig>,
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
            modifier: self.modifier,
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
    modifier: Option<PartialRateBoostConfig>,
}

impl CreditRateBoostConfig {
    fn from_raw(defaults: RateBoostDefaults, children: RawRateBoostChildren) -> Self {
        Self {
            key: RateBoostConfig::resolve(&defaults, children.key, default_key_baseline_per_sec()),
            click: RateBoostConfig::resolve(
                &defaults,
                children.click,
                default_click_baseline_per_sec(),
            ),
            scroll: RateBoostConfig::resolve(
                &defaults,
                children.scroll,
                default_scroll_baseline_per_sec(),
            ),
            drag: RateBoostConfig::resolve(
                &defaults,
                children.drag,
                default_drag_baseline_per_sec(),
            ),
            modifier: RateBoostConfig::resolve(
                &defaults,
                children.modifier,
                default_modifier_baseline_per_sec(),
            ),
            defaults,
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

#[derive(Debug, Clone)]
pub struct RateBoostConfig {
    pub enabled: bool,
    pub baseline_per_sec: f64,
    pub factor: f64,
    pub cap: f64,
    pub smoothing_secs: f64,
}

impl RateBoostConfig {
    fn resolve(
        defaults: &RateBoostDefaults,
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

fn default_unclassified_combo_cost() -> f64 {
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
    fn cost_config_resolves_hand_defaults_and_overrides() {
        let config: CreditCostConfig = toml::from_str(
            r#"
            [hand]
            key = 1.5
            same_hand_combo = 2.5

            [hand.modifier]
            shift_per_sec = 6.0

            [left]
            key = 1.1

            [left.modifier]
            ctrl_per_sec = 7.0

            [right]
            scroll = 0.30

            [unclassified]
            key = 2.0
            combo = 3.0
            "#,
        )
        .expect("cost config should parse");

        let resolved = config.resolve();
        resolved.validate().expect("resolved costs should validate");
        assert_close(resolved.left.key, 1.1);
        assert_close(resolved.right.key, 1.5);
        assert_close(resolved.left.same_hand_combo, 2.5);
        assert_close(resolved.right.scroll, 0.30);
        assert_close(resolved.left.modifier.shift_per_sec, 6.0);
        assert_close(resolved.right.modifier.shift_per_sec, 6.0);
        assert_close(resolved.left.modifier.ctrl_per_sec, 7.0);
        assert_close(resolved.right.modifier.ctrl_per_sec, 5.0);
        assert_close(resolved.unclassified.key, 2.0);
        assert_close(resolved.unclassified.combo, 3.0);
    }

    #[test]
    fn old_cost_sections_are_rejected() {
        let old_key: Result<CreditCostConfig, _> = toml::from_str(
            r#"
            [key]
            left = 1.0
            "#,
        );
        assert!(old_key.is_err(), "old key cost section should be rejected");

        let old_modifier: Result<CreditCostConfig, _> = toml::from_str(
            r#"
            [left_modifier]
            shift_per_sec = 5.0
            "#,
        );
        assert!(
            old_modifier.is_err(),
            "old modifier cost section should be rejected"
        );
    }

    #[test]
    fn invalid_resolved_cost_reports_field_path() {
        let costs: CreditCostConfig = toml::from_str(
            r#"
            [left]
            key = -1.0
            "#,
        )
        .expect("cost config should parse");
        let config = CreditCalculatorConfig::from_parts(Some(costs), None, None);

        let err = config
            .validate()
            .expect_err("negative cost should fail validation");
        assert!(
            err.to_string().contains("credit.costs.left.key"),
            "unexpected error: {err}"
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
    fn modifier_rate_boost_child_inherits_parent_defaults() {
        let config: CreditRateBoostConfig = toml::from_str(
            r#"
            enabled = false
            factor = 0.5
            cap = 2.5
            smoothing_secs = 4.0

            [modifier]
            baseline_per_sec = 0.9
            "#,
        )
        .expect("rate boost config should parse");

        assert!(!config.modifier.enabled);
        assert_close(config.modifier.baseline_per_sec, 0.9);
        assert_close(config.modifier.factor, 0.5);
        assert_close(config.modifier.cap, 2.5);
        assert_close(config.modifier.smoothing_secs, 4.0);
    }

    #[test]
    fn old_modifier_rate_boost_children_are_rejected() {
        let result: Result<CreditRateBoostConfig, _> = toml::from_str(
            r#"
            [left_modifier]
            baseline_per_sec = 0.3
            "#,
        );

        assert!(
            result.is_err(),
            "old modifier rate boost child should be rejected"
        );
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
