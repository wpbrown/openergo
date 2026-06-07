use super::super::config::Config;
use directories::ProjectDirs;
use rootcause::prelude::*;
use std::path::PathBuf;
use tracing::info;

fn default_path() -> PathBuf {
    ProjectDirs::from("", "", "openergo")
        .map(|dirs| dirs.config_dir().join("client.toml"))
        .unwrap_or_else(|| PathBuf::from("client.toml"))
}

pub fn run(config_path: Option<PathBuf>) -> Result<Config, Report> {
    let (config_path, explicit) = match config_path {
        Some(p) => (p, true),
        None => (default_path(), false),
    };
    if config_path.exists() {
        Ok(Config::load(&config_path).context("failed to load configuration")?)
    } else if explicit {
        bail!(
            "specified config file not found at {}",
            config_path.display()
        );
    } else {
        info!(
            "no config file found at {}, using defaults",
            config_path.display()
        );
        Ok(Config::default())
    }
}
