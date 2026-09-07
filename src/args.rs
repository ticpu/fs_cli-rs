//! Command-line argument parsing for fs_cli-rs

use crate::config::{AppConfig, FsCliConfig, ProfileConfig};
use crate::esl_debug::EslDebugLevel;
use crate::log_level::LogSetting;
use crate::printer::ColorMode;
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

/// Interactive FreeSWITCH CLI client
#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Profile to use from configuration file
    #[arg(value_name = "PROFILE")]
    pub profile: Option<String>,

    /// FreeSWITCH hostname or IP address
    #[arg(short = 'H', long)]
    pub host: Option<String>,

    /// FreeSWITCH ESL port
    #[arg(short = 'P', long)]
    pub port: Option<u16>,

    /// ESL password
    #[arg(short = 'p', long)]
    pub password: Option<String>,

    /// Username for userauth (format: user@domain, e.g., admin@default)
    #[arg(short, long)]
    pub user: Option<String>,

    /// ESL debug level (0-7, higher = more verbose)
    #[arg(short, long)]
    pub debug: Option<u8>,

    /// Color mode for output (never, tag, line)
    #[arg(long, ignore_case = true)]
    pub color: Option<ColorMode>,

    /// Execute commands and exit (can be used multiple times)
    #[arg(short = 'x', action = clap::ArgAction::Append)]
    pub execute: Vec<String>,

    /// Write the FreeSWITCH log stream to PATH ("-" for stdout)
    #[arg(long, value_name = "PATH")]
    pub log_file: Option<String>,

    /// History file path
    #[arg(long)]
    pub history_file: Option<PathBuf>,

    /// Connection timeout in milliseconds
    #[arg(short = 'T', long = "connect-timeout")]
    pub timeout: Option<u64>,

    /// Retry connection on failure
    #[arg(short, long, num_args = 0..=1, default_missing_value = "true", action = clap::ArgAction::Set)]
    pub retry: Option<bool>,

    /// Reconnect on connection loss
    #[arg(short = 'R', long, num_args = 0..=1, default_missing_value = "true", action = clap::ArgAction::Set)]
    pub reconnect: Option<bool>,

    /// Subscribe to events on startup
    #[arg(long, num_args = 0..=1, default_missing_value = "true", action = clap::ArgAction::Set)]
    pub events: Option<bool>,

    /// Log level for FreeSWITCH logs
    #[arg(short = 'l', long)]
    pub log_level: Option<LogSetting>,

    /// Disable automatic log subscription on startup
    #[arg(short = 'q', long, num_args = 0..=1, default_missing_value = "true", action = clap::ArgAction::Set)]
    pub quiet: Option<bool>,

    /// Configuration file path (if missing, a default is written)
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// List available configuration profiles
    #[arg(long)]
    pub list_profiles: bool,
}

impl Args {
    /// Parse arguments and merge with configuration
    pub fn parse_and_merge() -> Result<AppConfig> {
        let args = Self::parse();

        let config = FsCliConfig::load(
            args.config
                .clone(),
        )?;

        if args.list_profiles {
            Self::print_profiles_and_exit(&config);
        }

        let mut app_config = Self::resolve_profile(
            &config,
            args.profile
                .as_deref(),
        )?;
        args.apply_to(&mut app_config)?;
        Ok(app_config)
    }

    /// `--list-profiles`: print every configured profile name and exit.
    fn print_profiles_and_exit(config: &FsCliConfig) -> ! {
        println!("Available profiles:");
        let mut profile_names = config.get_profile_names();
        profile_names.sort();
        for name in profile_names {
            println!("  {}", name);
        }
        std::process::exit(0);
    }

    /// The named profile, or the default profile when none was named, or an
    /// error listing what is available when a named one does not exist.
    fn resolve_profile(config: &FsCliConfig, profile: Option<&str>) -> Result<AppConfig> {
        let profile_name = profile.unwrap_or("default");
        let explicitly_named = profile.is_some();

        match config.get_profile(profile_name) {
            Ok(profile) => Ok(profile.into_app_config()),
            Err(_) if !explicitly_named => Ok(ProfileConfig::default().into_app_config()),
            Err(_) => {
                let mut names = config.get_profile_names();
                names.sort();
                Err(anyhow::anyhow!(
                    "Profile '{}' not found. Available profiles: {}",
                    profile_name,
                    names.join(", ")
                ))
            }
        }
    }

    /// Apply CLI argument overrides to an already-loaded AppConfig.
    // qual:allow(complexity, max_cyclomatic=14) reason: "flat sequence of one if-let override per CLI flag; the clearest form, splitting would scatter it"
    pub fn apply_to(&self, config: &mut AppConfig) -> Result<()> {
        if let Some(host) = &self.host {
            config.host = host.clone();
        }
        if let Some(port) = self.port {
            config.port = port;
        }
        if let Some(password) = &self.password {
            config.password = password.clone();
        }
        if let Some(user) = &self.user {
            config.user = Some(user.clone());
        }
        if let Some(debug) = self.debug {
            config.debug = EslDebugLevel::try_from(debug)?;
        }
        if let Some(color) = self.color {
            config.color = color;
        }
        if let Some(log_file) = &self.log_file {
            config.log_file = Some(log_file.clone());
        }
        if let Some(history_file) = &self.history_file {
            config.history_file = Some(history_file.clone());
        }
        if let Some(timeout) = self.timeout {
            config.timeout = timeout;
        }
        if let Some(retry) = self.retry {
            config.retry = retry;
        }
        if let Some(reconnect) = self.reconnect {
            config.reconnect = reconnect;
        }
        if let Some(events) = self.events {
            config.events = events;
        }
        if let Some(log_level) = self.log_level {
            config.log_level = log_level;
        }
        if let Some(quiet) = self.quiet {
            config.quiet = quiet;
        }
        config.execute = self
            .execute
            .clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Args;
    use crate::config::AppConfig;
    use crate::esl_debug::EslDebugLevel;
    use crate::log_level::LogSetting;
    use crate::printer::ColorMode;
    use freeswitch_esl_tokio::LogLevel;
    use std::collections::HashMap;

    /// The YAML side parses case-insensitively; the flag must agree with it.
    #[test]
    fn color_accepts_any_case() {
        use clap::Parser;
        for spelling in ["never", "NEVER", "Never"] {
            let args = Args::try_parse_from(["fs_cli", "--color", spelling]).unwrap();
            assert_eq!(args.color, Some(ColorMode::Never));
        }
        assert!(Args::try_parse_from(["fs_cli", "--color", "rainbow"]).is_err());
    }

    fn make_args_no_overrides() -> Args {
        Args {
            profile: None,
            host: None,
            port: None,
            password: None,
            user: None,
            debug: None,
            color: None,
            execute: Vec::new(),
            log_file: None,
            history_file: None,
            timeout: None,
            retry: None,
            reconnect: None,
            events: None,
            log_level: None,
            quiet: None,
            config: None,
            list_profiles: false,
        }
    }

    fn base_app_config() -> AppConfig {
        AppConfig {
            host: "localhost".to_string(),
            port: 8021,
            password: "test".to_string(),
            user: None,
            debug: EslDebugLevel::None,
            color: ColorMode::Line,
            log_file: None,
            history_file: None,
            timeout: 2000,
            retry: true,
            reconnect: true,
            events: true,
            log_level: LogSetting::Level(LogLevel::Debug),
            quiet: true,
            macros: HashMap::new(),
            execute: Vec::new(),
            max_auto_complete_uuid: 32,
        }
    }

    #[test]
    fn test_apply_to_preserves_config_when_no_cli_overrides() {
        let mut config = base_app_config();
        make_args_no_overrides()
            .apply_to(&mut config)
            .unwrap();
        assert!(config.retry);
        assert!(config.reconnect);
        assert!(config.events);
        assert!(config.quiet);
    }

    #[test]
    fn test_apply_to_overrides_config_with_cli_args() {
        let mut config = base_app_config();
        config.retry = false;
        config.reconnect = false;
        config.events = false;
        config.quiet = false;

        let mut args = make_args_no_overrides();
        args.retry = Some(true);
        args.reconnect = Some(true);
        args.events = Some(true);
        args.quiet = Some(true);

        args.apply_to(&mut config)
            .unwrap();
        assert!(config.retry);
        assert!(config.reconnect);
        assert!(config.events);
        assert!(config.quiet);
    }

    #[test]
    fn test_apply_to_host_and_port_override() {
        let mut config = base_app_config();
        let mut args = make_args_no_overrides();
        args.host = Some("192.168.1.1".to_string());
        args.port = Some(9021);

        args.apply_to(&mut config)
            .unwrap();
        assert_eq!(config.host, "192.168.1.1");
        assert_eq!(config.port, 9021);
    }

    #[test]
    fn test_apply_to_execute_always_replaced() {
        let mut config = base_app_config();
        config.execute = vec!["prior".to_string()];

        let mut args = make_args_no_overrides();
        args.execute = vec!["status".to_string(), "version".to_string()];

        args.apply_to(&mut config)
            .unwrap();
        assert_eq!(config.execute, vec!["status", "version"]);
    }
}
