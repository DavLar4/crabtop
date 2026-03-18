/// config.rs — Configuration loading and defaults

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Color theme name
    pub theme: String,
    /// Update interval in ms
    pub update_interval_ms: u64,
    /// Show swap in memory box
    pub show_swap: bool,
    /// Show disk IO stats
    pub show_io_stat: bool,
    /// Process sort field
    pub proc_sorting: ProcSort,
    /// Reverse process sort order
    pub proc_reversed: bool,
    /// Show process tree view
    pub proc_tree: bool,
    /// Show per-CPU graph
    pub cpu_single_graph: bool,
    /// Show network graphs in auto-scale mode
    pub net_auto: bool,
    /// Layout preset index (0–9)
    pub preset: u8,
    /// Use vim-style keys
    pub vim_keys: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProcSort {
    Cpu,
    Memory,
    Pid,
    Name,
    Threads,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "default".into(),
            update_interval_ms: 2000,
            show_swap: true,
            show_io_stat: true,
            proc_sorting: ProcSort::Cpu,
            proc_reversed: false,
            proc_tree: false,
            cpu_single_graph: false,
            net_auto: true,
            preset: 0,
            vim_keys: false,
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let config_path = match path {
            Some(p) => p.to_owned(),
            None => default_config_path()?,
        };

        if !config_path.exists() {
            debug!("No config file at {}, using defaults", config_path.display());
            return Ok(Self::default());
        }

        let raw = std::fs::read_to_string(&config_path)?;
        let cfg: Config = toml::from_str(&raw)?;
        debug!("Loaded config from {}", config_path.display());
        Ok(cfg)
    }

    #[allow(dead_code)]
    pub fn save(&self, path: Option<&Path>) -> Result<()> {
        let config_path = match path {
            Some(p) => p.to_owned(),
            None => default_config_path()?,
        };

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let toml_str = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, toml_str)?;
        debug!("Saved config to {}", config_path.display());
        Ok(())
    }
}

fn default_config_path() -> Result<PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config")
    } else {
        warn!("Neither XDG_CONFIG_HOME nor HOME set, using /tmp");
        PathBuf::from("/tmp")
    };
    Ok(base.join("crabtop").join("crabtop.toml"))
}
