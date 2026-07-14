use rootcause::prelude::*;

#[derive(Debug, Clone, Default)]
pub struct CreditCalculatorConfig {
    pub costs: ResolvedCreditCosts,
    pub rate_boost: CreditRateBoostConfig,
    pub global_boost: GlobalCreditBoostConfig,
}

impl CreditCalculatorConfig {
    pub fn validate(&self) -> Result<(), Report> {
        self.costs.validate()?;
        self.rate_boost.validate()?;
        self.global_boost.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
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

#[derive(Debug, Clone)]
pub struct UnclassifiedCostConfig {
    pub key: f64,
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

#[derive(Debug, Clone)]
pub struct ModifierCostConfig {
    pub shift_per_sec: f64,
    pub ctrl_per_sec: f64,
    pub alt_per_sec: f64,
    pub meta_per_sec: f64,
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
    pub key: RateBoostConfig,
    pub click: RateBoostConfig,
    pub scroll: RateBoostConfig,
    pub drag: RateBoostConfig,
    pub modifier: RateBoostConfig,
}

impl CreditRateBoostConfig {
    fn validate(&self) -> Result<(), Report> {
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
        Self {
            key: RateBoostConfig::default_with_baseline(default_key_baseline_per_sec()),
            click: RateBoostConfig::default_with_baseline(default_click_baseline_per_sec()),
            scroll: RateBoostConfig::default_with_baseline(default_scroll_baseline_per_sec()),
            drag: RateBoostConfig::default_with_baseline(default_drag_baseline_per_sec()),
            modifier: RateBoostConfig::default_with_baseline(default_modifier_baseline_per_sec()),
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
    fn default_with_baseline(baseline_per_sec: f64) -> Self {
        Self {
            enabled: default_rate_enabled(),
            baseline_per_sec,
            factor: default_rate_factor(),
            cap: default_rate_cap(),
            smoothing_secs: default_rate_smoothing_secs(),
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

#[derive(Debug, Clone)]
pub struct GlobalCreditBoostConfig {
    pub enabled: bool,
    pub baseline_credit_per_sec: f64,
    pub factor: f64,
    pub cap: f64,
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
