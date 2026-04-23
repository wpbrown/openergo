use directories::ProjectDirs;
use rootcause::prelude::*;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub telemetry: Option<TelemetryConfig>,
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

impl Config {
    pub fn telemetry(&self) -> Option<&TelemetryConfig> {
        self.telemetry.as_ref()
    }

    pub fn load(path: &Path) -> Result<Self, Report> {
        let content = std::fs::read_to_string(path)
            .context("Failed to read config file")
            .attach(format!("path: {}", path.display()))?;
        let config: Config = toml::from_str(&content).context("Failed to parse config file")?;
        log::info!("Loaded config from {}", path.display());
        Ok(config)
    }

    pub fn default_path() -> PathBuf {
        ProjectDirs::from("", "", "openergo")
            .map(|dirs| dirs.config_dir().join("client.toml"))
            .unwrap_or_else(|| PathBuf::from("client.toml"))
    }
}

