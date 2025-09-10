//! Configuration management for fs_cli-rs

use crate::commands::{ColorMode, LogLevel};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Top-level configuration structure matching the YAML format
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FsCliConfig {
    pub fs_cli: HashMap<String, ProfileConfig>,
}

/// Configuration for a single profile
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ProfileConfig {
    /// FreeSWITCH hostname or IP address
    pub host: Option<String>,

    /// FreeSWITCH ESL port
    pub port: Option<u16>,

    /// ESL password
    pub password: Option<String>,

    /// Username for authentication (optional)
    pub user: Option<String>,

    /// ESL debug level (0-7, higher = more verbose)
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

    /// Disable automatic log subscription on startup
    pub quiet: Option<bool>,

    /// Function key macros
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
            macros: Some(Self::default_macros()),
        }
    }
}

impl ProfileConfig {
    /// Get default macros
    fn default_macros() -> HashMap<String, String> {
        let mut macros = HashMap::new();
        macros.insert("f1".to_string(), "help".to_string());
        macros.insert("f2".to_string(), "status".to_string());
        macros.insert("f3".to_string(), "show channels".to_string());
        macros.insert("f4".to_string(), "show calls".to_string());
        macros.insert("f5".to_string(), "sofia status".to_string());
        macros.insert("f6".to_string(), "reloadxml".to_string());
        macros.insert("f7".to_string(), "/log console".to_string());
        macros.insert("f8".to_string(), "/log debug".to_string());
        macros.insert(
            "f9".to_string(),
            "sofia status profile internal".to_string(),
        );
        macros.insert("f10".to_string(), "fsctl pause".to_string());
        macros.insert("f11".to_string(), "fsctl resume".to_string());
        macros.insert("f12".to_string(), "version".to_string());
        macros
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
            debug: crate::esl_debug::EslDebugLevel::from_u8(self.debug.unwrap_or(0))?,
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
    pub debug: crate::esl_debug::EslDebugLevel,
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

impl FsCliConfig {
    /// Load configuration from file or create default
    pub fn load(config_path: Option<PathBuf>) -> Result<Self> {
        let config_paths = if let Some(path) = config_path {
            vec![path]
        } else {
            Self::get_default_config_paths()
        };

        // Try to load from existing config files
        for path in &config_paths {
            if path.exists() {
                let content = std::fs::read_to_string(path).map_err(|e| {
                    anyhow::anyhow!("Failed to read config file {}: {}", path.display(), e)
                })?;
                let config: Self = serde_yaml::from_str(&content).map_err(|e| {
                    anyhow::anyhow!("Failed to parse config file {}: {}", path.display(), e)
                })?;
                return Ok(config);
            }
        }

        // No existing config found, create default
        let default_config = Self::default();

        // Create the config file if we have a writable directory
        if let Some(config_dir) = dirs::config_dir() {
            let config_path = config_dir.join("fs_cli.yaml");
            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent).ok(); // Ignore errors
                let yaml_content = serde_yaml::to_string(&default_config).unwrap_or_default();
                std::fs::write(&config_path, yaml_content).ok(); // Ignore errors
            }
        }

        Ok(default_config)
    }

    /// Get list of default configuration file paths to try
    fn get_default_config_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();

        // User config directory (~/.config/fs_cli.yaml)
        if let Some(config_dir) = dirs::config_dir() {
            paths.push(config_dir.join("fs_cli.yaml"));
        }

        // User home directory (~/.fs_cli.yaml)
        if let Some(home_dir) = dirs::home_dir() {
            paths.push(home_dir.join(".fs_cli.yaml"));
        }

        // System-wide config (/etc/freeswitch/fs_cli.yaml)
        paths.push(PathBuf::from("/etc/freeswitch/fs_cli.yaml"));

        paths
    }

    /// Get a profile by name
    pub fn get_profile(&self, name: &str) -> Result<ProfileConfig> {
        self.fs_cli
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Profile '{}' not found", name))
    }

    /// Get list of available profile names
    pub fn get_profile_names(&self) -> Vec<String> {
        self.fs_cli.keys().cloned().collect()
    }
}

impl Default for FsCliConfig {
    fn default() -> Self {
        let mut fs_cli = HashMap::new();
        fs_cli.insert("default".to_string(), ProfileConfig::default());

        Self { fs_cli }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_boolean_config_parsing() {
        let yaml_content = r#"
fs_cli:
  test_true:
    host: localhost
    retry: true
    reconnect: true
    events: true
    quiet: true
  test_false:
    host: localhost
    retry: false
    reconnect: false
    events: false
    quiet: false
"#;

        let config: FsCliConfig = serde_yaml::from_str(yaml_content).unwrap();

        // Test true profile
        let true_profile = config.get_profile("test_true").unwrap();
        assert_eq!(true_profile.retry, Some(true));
        assert_eq!(true_profile.reconnect, Some(true));
        assert_eq!(true_profile.events, Some(true));
        assert_eq!(true_profile.quiet, Some(true));

        // Test false profile
        let false_profile = config.get_profile("test_false").unwrap();
        assert_eq!(false_profile.retry, Some(false));
        assert_eq!(false_profile.reconnect, Some(false));
        assert_eq!(false_profile.events, Some(false));
        assert_eq!(false_profile.quiet, Some(false));

        // Test to_app_config conversion
        let true_app_config = true_profile.to_app_config().unwrap();
        assert_eq!(true_app_config.retry, true);
        assert_eq!(true_app_config.reconnect, true);
        assert_eq!(true_app_config.events, true);
        assert_eq!(true_app_config.quiet, true);

        let false_app_config = false_profile.to_app_config().unwrap();
        assert_eq!(false_app_config.retry, false);
        assert_eq!(false_app_config.reconnect, false);
        assert_eq!(false_app_config.events, false);
        assert_eq!(false_app_config.quiet, false);
    }

    #[test]
    fn test_profile_merging_with_cli_args() {
        // Simulate CLI args behavior
        struct MockCliArgs {
            retry: Option<bool>,
            reconnect: Option<bool>,
            events: Option<bool>,
            quiet: Option<bool>,
        }

        // Test 1: Config has true values, no CLI override
        let mut config = AppConfig {
            host: "localhost".to_string(),
            port: 8021,
            password: "test".to_string(),
            user: None,
            debug: crate::esl_debug::EslDebugLevel::None,
            color: ColorMode::Line,
            history_file: None,
            timeout: 2000,
            retry: true,
            reconnect: true,
            events: true,
            log_level: LogLevel::Debug,
            quiet: true,
            macros: HashMap::new(),
            execute: Vec::new(),
        };

        let cli_args = MockCliArgs {
            retry: None,
            reconnect: None,
            events: None,
            quiet: None,
        };

        // Simulate the merging logic from args.rs
        if let Some(retry) = cli_args.retry {
            config.retry = retry;
        }
        if let Some(reconnect) = cli_args.reconnect {
            config.reconnect = reconnect;
        }
        if let Some(events) = cli_args.events {
            config.events = events;
        }
        if let Some(quiet) = cli_args.quiet {
            config.quiet = quiet;
        }

        // Config values should remain unchanged
        assert_eq!(config.retry, true);
        assert_eq!(config.reconnect, true);
        assert_eq!(config.events, true);
        assert_eq!(config.quiet, true);

        // Test 2: Config has false values, CLI overrides to true
        config.retry = false;
        config.reconnect = false;
        config.events = false;
        config.quiet = false;

        let cli_args_override = MockCliArgs {
            retry: Some(true),
            reconnect: Some(true),
            events: Some(true),
            quiet: Some(true),
        };

        if let Some(retry) = cli_args_override.retry {
            config.retry = retry;
        }
        if let Some(reconnect) = cli_args_override.reconnect {
            config.reconnect = reconnect;
        }
        if let Some(events) = cli_args_override.events {
            config.events = events;
        }
        if let Some(quiet) = cli_args_override.quiet {
            config.quiet = quiet;
        }

        // CLI should override config
        assert_eq!(config.retry, true);
        assert_eq!(config.reconnect, true);
        assert_eq!(config.events, true);
        assert_eq!(config.quiet, true);
    }
}
