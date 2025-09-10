//! Configuration management for fs_cli-rs
//!
//! Handles loading and merging configuration from YAML files and command-line arguments.
//! Supports profiles for different environments (default, prod, dev, etc.).

use crate::commands::{ColorMode, LogLevel};
use crate::esl_debug::EslDebugLevel;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Embedded default configuration
const DEFAULT_CONFIG: &str = include_str!("../fs_cli.yaml");

/// Configuration structure for fs_cli
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsCliConfig {
    /// Configuration profiles (default, prod, dev, etc.)
    pub fs_cli: HashMap<String, ProfileConfig>,
}

/// Configuration for a specific profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    /// FreeSWITCH hostname or IP address
    pub host: Option<String>,

    /// FreeSWITCH ESL port
    pub port: Option<u16>,

    /// ESL password
    pub password: Option<String>,

    /// Username for authentication
    pub user: Option<String>,

    /// ESL debug level (0-7)
    pub debug: Option<u8>,

    /// Color mode for output
    pub color: Option<String>,

    /// History file path
    pub history_file: Option<String>,

    /// Connection timeout in milliseconds
    pub timeout: Option<u64>,

    /// Retry connection on failure
    pub retry: Option<bool>,

    /// Reconnect on connection loss
    pub reconnect: Option<bool>,

    /// Subscribe to events on startup
    pub events: Option<bool>,

    /// Log level for FreeSWITCH logs
    pub log_level: Option<String>,

    /// Disable automatic log subscription
    pub quiet: Option<bool>,

    /// Custom function key macros
    pub macros: Option<HashMap<String, String>>,
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            host: Some("localhost".to_string()),
            port: Some(8021),
            password: Some("ClueCon".to_string()),
            user: None,
            debug: Some(0),
            color: Some("line".to_string()),
            history_file: None,
            timeout: Some(2000),
            retry: Some(false),
            reconnect: Some(false),
            events: Some(false),
            log_level: Some("debug".to_string()),
            quiet: Some(false),
            macros: None,
        }
    }
}

impl FsCliConfig {
    /// Load configuration from file paths with optional custom config path
    pub fn load(custom_config_path: Option<PathBuf>) -> Result<Self> {
        let config_paths = if let Some(custom_path) = custom_config_path {
            vec![custom_path]
        } else {
            Self::get_config_paths()
        };

        for path in config_paths {
            if path.exists() {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read config file: {}", path.display()))?;

                let config: FsCliConfig = serde_yaml::from_str(&content)
                    .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

                return Ok(config);
            } else if path.parent().is_some_and(|p| p.exists()) {
                // Directory exists but file doesn't - create it from embedded config
                eprintln!("Creating default configuration file: {}", path.display());
                Self::create_default_config_file(&path)?;

                // Now load the newly created file
                let content = std::fs::read_to_string(&path).with_context(|| {
                    format!(
                        "Failed to read newly created config file: {}",
                        path.display()
                    )
                })?;

                let config: FsCliConfig = serde_yaml::from_str(&content).with_context(|| {
                    format!(
                        "Failed to parse newly created config file: {}",
                        path.display()
                    )
                })?;

                return Ok(config);
            }
        }

        // No config file found and couldn't create one, return default configuration
        Ok(Self::default_config())
    }

    /// Create a default configuration file from embedded template
    fn create_default_config_file(path: &PathBuf) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }

        std::fs::write(path, DEFAULT_CONFIG)
            .with_context(|| format!("Failed to write default config file: {}", path.display()))?;

        Ok(())
    }

    /// Get potential configuration file paths in order of preference
    fn get_config_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();

        // ~/.config/fs_cli.yaml
        if let Some(config_dir) = dirs::config_dir() {
            paths.push(config_dir.join("fs_cli.yaml"));
        }

        // ~/.fs_cli.yaml
        if let Some(home_dir) = dirs::home_dir() {
            paths.push(home_dir.join(".fs_cli.yaml"));
        }

        // /etc/freeswitch/fs_cli.yaml
        paths.push(PathBuf::from("/etc/freeswitch/fs_cli.yaml"));

        paths
    }

    /// Create a default configuration
    fn default_config() -> Self {
        let mut profiles = HashMap::new();
        profiles.insert("default".to_string(), ProfileConfig::default());

        Self { fs_cli: profiles }
    }

    /// Get configuration for a specific profile
    pub fn get_profile(&self, profile_name: &str) -> Result<ProfileConfig> {
        // Start with default profile if it exists
        let mut config = if let Some(default) = self.fs_cli.get("default") {
            default.clone()
        } else {
            ProfileConfig::default()
        };

        // If requesting a specific profile, merge it with default
        if profile_name != "default" {
            if let Some(profile) = self.fs_cli.get(profile_name) {
                config = Self::merge_profiles(config, profile.clone());
            } else {
                return Err(anyhow::anyhow!(
                    "Profile '{}' not found in configuration",
                    profile_name
                ));
            }
        }

        Ok(config)
    }

    /// Merge two profiles, with override taking precedence
    fn merge_profiles(mut base: ProfileConfig, override_profile: ProfileConfig) -> ProfileConfig {
        if override_profile.host.is_some() {
            base.host = override_profile.host;
        }
        if override_profile.port.is_some() {
            base.port = override_profile.port;
        }
        if override_profile.password.is_some() {
            base.password = override_profile.password;
        }
        if override_profile.user.is_some() {
            base.user = override_profile.user;
        }
        if override_profile.debug.is_some() {
            base.debug = override_profile.debug;
        }
        if override_profile.color.is_some() {
            base.color = override_profile.color;
        }
        if override_profile.history_file.is_some() {
            base.history_file = override_profile.history_file;
        }
        if override_profile.timeout.is_some() {
            base.timeout = override_profile.timeout;
        }
        if override_profile.retry.is_some() {
            base.retry = override_profile.retry;
        }
        if override_profile.reconnect.is_some() {
            base.reconnect = override_profile.reconnect;
        }
        if override_profile.events.is_some() {
            base.events = override_profile.events;
        }
        if override_profile.log_level.is_some() {
            base.log_level = override_profile.log_level;
        }
        if override_profile.quiet.is_some() {
            base.quiet = override_profile.quiet;
        }
        if override_profile.macros.is_some() {
            base.macros = override_profile.macros;
        }

        base
    }

    /// Get available profile names
    pub fn get_profile_names(&self) -> Vec<String> {
        self.fs_cli.keys().cloned().collect()
    }
}

impl ProfileConfig {
    /// Convert to typed values for application use
    pub fn to_app_config(&self) -> Result<AppConfig> {
        Ok(AppConfig {
            host: self.host.clone().unwrap_or_else(|| "localhost".to_string()),
            port: self.port.unwrap_or(8021),
            password: self
                .password
                .clone()
                .unwrap_or_else(|| "ClueCon".to_string()),
            user: self.user.clone(),
            debug: EslDebugLevel::from_u8(self.debug.unwrap_or(0))?,
            color: self
                .color
                .as_deref()
                .unwrap_or("line")
                .parse::<ColorMode>()
                .map_err(|e| anyhow::anyhow!("Invalid color mode: {}", e))?,
            history_file: self.history_file.as_ref().map(PathBuf::from),
            timeout: self.timeout.unwrap_or(2000),
            retry: self.retry.unwrap_or(false),
            reconnect: self.reconnect.unwrap_or(false),
            events: self.events.unwrap_or(false),
            log_level: self
                .log_level
                .as_deref()
                .unwrap_or("debug")
                .parse::<LogLevel>()
                .map_err(|e| anyhow::anyhow!("Invalid log level: {}", e))?,
            quiet: self.quiet.unwrap_or(false),
            macros: self.macros.clone().unwrap_or_default(),
            execute: Vec::new(), // Always empty from config, filled by CLI args
        })
    }
}

/// Typed application configuration after parsing and validation
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub password: String,
    pub user: Option<String>,
    pub debug: EslDebugLevel,
    pub color: ColorMode,
    pub history_file: Option<PathBuf>,
    pub timeout: u64,
    pub retry: bool,
    pub reconnect: bool,
    pub events: bool,
    pub log_level: LogLevel,
    pub quiet: bool,
    pub macros: HashMap<String, String>,
    pub execute: Vec<String>,
}
