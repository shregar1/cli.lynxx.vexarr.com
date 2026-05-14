//! Local config management (`~/.config/lynxx/config.toml`).

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{bold, dim, ConfigCmd};

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LocalConfig {
    #[serde(default)]
    pub gateway_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub license_key: Option<String>,
    #[serde(default)]
    pub secret_key: Option<String>,
    #[serde(default)]
    pub telemetry_url: Option<String>,
    #[serde(default)]
    pub telemetry_api_key: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub no_color: Option<bool>,
    #[serde(default)]
    pub full_id: Option<bool>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().ok_or_else(|| anyhow!("could not determine user config dir"))?;
    Ok(base.join("lynxx").join("config.toml"))
}

pub fn load_config() -> LocalConfig {
    let path = match config_path() {
        Ok(p) => p,
        Err(_) => return LocalConfig::default(),
    };
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return LocalConfig::default(),
    };
    toml::from_str(&raw).unwrap_or_default()
}

pub fn save_config(cfg: &LocalConfig) -> Result<PathBuf> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let raw = toml::to_string_pretty(cfg)?;
    fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// CLI handler
// ---------------------------------------------------------------------------

pub fn cmd_config(action: ConfigCmd) -> Result<()> {
    match action {
        ConfigCmd::Path => {
            println!("{}", config_path()?.display());
        }
        ConfigCmd::Show => {
            let path = config_path()?;
            match fs::read_to_string(&path) {
                Ok(raw) => print!("{raw}"),
                Err(_) => println!("{}", dim(&format!("(no config at {})", path.display()))),
            }
        }
        ConfigCmd::Get { key } => {
            let cfg = load_config();
            let val: Option<String> = match key.as_str() {
                "gateway_url" => cfg.gateway_url,
                "api_key" => cfg.api_key,
                "base_url" => cfg.base_url,
                "license_key" => cfg.license_key,
                "secret_key" => cfg.secret_key,
                "telemetry_url" => cfg.telemetry_url,
                "telemetry_api_key" => cfg.telemetry_api_key,
                "output" => cfg.output,
                "no_color" => cfg.no_color.map(|b| b.to_string()),
                "full_id" => cfg.full_id.map(|b| b.to_string()),
                other => bail!("unknown config key '{other}'"),
            };
            match val {
                Some(v) => println!("{v}"),
                None => println!("{}", dim("(unset)")),
            }
        }
        ConfigCmd::Set { key, value } => {
            let mut cfg = load_config();
            match key.as_str() {
                "gateway_url" => cfg.gateway_url = Some(value.clone()),
                "api_key" => cfg.api_key = Some(value.clone()),
                "base_url" => cfg.base_url = Some(value.clone()),
                "license_key" => cfg.license_key = Some(value.clone()),
                "secret_key" => cfg.secret_key = Some(value.clone()),
                "telemetry_url" => cfg.telemetry_url = Some(value.clone()),
                "telemetry_api_key" => cfg.telemetry_api_key = Some(value.clone()),
                "output" => {
                    if !["table", "json", "yaml"].contains(&value.as_str()) {
                        bail!("output must be one of: table, json, yaml");
                    }
                    cfg.output = Some(value.clone());
                }
                "no_color" | "full_id" => {
                    let b = match value.as_str() {
                        "true" | "1" | "yes" => true,
                        "false" | "0" | "no" => false,
                        _ => bail!("{key} must be true/false"),
                    };
                    match key.as_str() {
                        "no_color" => cfg.no_color = Some(b),
                        _ => cfg.full_id = Some(b),
                    }
                }
                other => bail!("unknown config key '{other}'"),
            }
            let path = save_config(&cfg)?;
            println!(
                "{} set {}={} ({})",
                bold("✓"),
                key,
                value,
                dim(&path.display().to_string())
            );
        }
    }
    Ok(())
}
