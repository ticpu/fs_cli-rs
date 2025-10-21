//! Command-line argument parsing for fs_cli-rs

use crate::commands::{ColorMode, LogLevel};
use crate::config::{AppConfig, FsCliConfig};
use crate::esl_debug::EslDebugLevel;
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

    /// Username for authentication (optional)
    #[arg(short, long)]
    pub user: Option<String>,

    /// ESL debug level (0-7, higher = more verbose)
    #[arg(short, long)]
    pub debug: Option<u8>,

    /// Color mode for output (never, tag, line)
    #[arg(long)]
    pub color: Option<ColorMode>,

    /// Execute commands and exit (can be used multiple times)
    #[arg(short = 'x', action = clap::ArgAction::Append)]
    pub execute: Vec<String>,

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
    pub log_level: Option<LogLevel>,

    /// Disable automatic log subscription on startup
    #[arg(short = 'q', long, num_args = 0..=1, default_missing_value = "true", action = clap::ArgAction::Set)]
    pub quiet: Option<bool>,

    /// Configuration file path (if missing, creates from embedded example)
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

        // Load configuration
        let config = FsCliConfig::load(args.config.clone())?;

        // Handle --list-profiles
        if args.list_profiles {
            println!("Available profiles:");
            let mut profile_names = config.get_profile_names();
            profile_names.sort();
            for name in profile_names {
                println!("  {}", name);
            }
            std::process::exit(0);
        }

        // Get profile name (default to "default")
        let profile_name = args.profile.as_deref().unwrap_or("default");

        // Load profile configuration
        let mut app_config = match config.get_profile(profile_name) {
            Ok(profile) => profile.to_app_config()?,
            Err(_) => {
                // If profile doesn't exist, create a default config and warn
                eprintln!(
                    "Warning: Profile '{}' not found, using defaults",
                    profile_name
                );
                config
                    .get_profile("default")
                    .unwrap_or_else(|_| crate::config::ProfileConfig::default())
                    .to_app_config()?
            }
        };

        // Override with command-line arguments
        if let Some(host) = args.host {
            app_config.host = host;
        }
        if let Some(port) = args.port {
            app_config.port = port;
        }
        if let Some(password) = args.password {
            app_config.password = password;
        }
        if let Some(user) = args.user {
            app_config.user = Some(user);
        }
        if let Some(debug) = args.debug {
            app_config.debug = EslDebugLevel::from_u8(debug)?;
        }
        if let Some(color) = args.color {
            app_config.color = color;
        }
        if let Some(history_file) = args.history_file {
            app_config.history_file = Some(history_file);
        }
        if let Some(timeout) = args.timeout {
            app_config.timeout = timeout;
        }
        if let Some(retry) = args.retry {
            app_config.retry = retry;
        }
        if let Some(reconnect) = args.reconnect {
            app_config.reconnect = reconnect;
        }
        if let Some(events) = args.events {
            app_config.events = events;
        }
        if let Some(log_level) = args.log_level {
            app_config.log_level = log_level;
        }
        if let Some(quiet) = args.quiet {
            app_config.quiet = quiet;
        }

        // Execute commands always come from CLI args
        app_config.execute = args.execute;

        Ok(app_config)
    }
}
