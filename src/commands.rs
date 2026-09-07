//! Command processing and execution for fs_cli-rs

use crate::esl_debug::EslDebugLevel;
use crate::log_level::{set_log_level, LogSetting};
use crate::printer::Printer;
use anyhow::{Error, Result};
use colored::*;
use freeswitch_esl_tokio::{CommandFailure, EslClient, EslError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::str::FromStr;

/// Color mode for log display
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorMode {
    Never,
    Tag,
    Line,
}

impl FromStr for ColorMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s
            .to_lowercase()
            .as_str()
        {
            "never" => Ok(ColorMode::Never),
            "tag" => Ok(ColorMode::Tag),
            "line" => Ok(ColorMode::Line),
            _ => Err(format!(
                "Invalid color mode: {}. Valid options: never, tag, line",
                s
            )),
        }
    }
}

impl std::fmt::Display for ColorMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ColorMode::Never => write!(f, "never"),
            ColorMode::Tag => write!(f, "tag"),
            ColorMode::Line => write!(f, "line"),
        }
    }
}

impl Serialize for ColorMode {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ColorMode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Command processor for FreeSWITCH CLI commands
pub struct CommandProcessor {
    color_mode: ColorMode,
    debug_level: EslDebugLevel,
    printer: Printer,
}

impl CommandProcessor {
    /// Create new command processor
    pub fn new(color_mode: ColorMode, debug_level: EslDebugLevel) -> Self {
        Self {
            color_mode,
            debug_level,
            printer: Printer::none(),
        }
    }

    /// Check if colors should be disabled
    fn no_color(&self) -> bool {
        self.color_mode == ColorMode::Never
    }

    /// Set external printer for coordinated output
    pub fn set_printer(&mut self, printer: Printer) {
        self.printer = printer;
    }

    fn print_message(&self, message: &str) {
        self.printer
            .print(message.to_string());
    }

    fn print_error(&self, message: &str) {
        self.printer
            .print_err(message.to_string());
    }

    fn labeled_error(&self, label: &str, message: &str) -> String {
        if self.no_color() {
            format!("{}: {}", label, message)
        } else {
            format!(
                "{}: {}",
                label
                    .red()
                    .bold(),
                message
            )
        }
    }

    /// Handle command execution errors with proper formatting
    pub fn handle_error(&self, error: Error) {
        let message = self.labeled_error("Error", &format!("{:#}", error));
        self.print_error(&message);
    }

    /// Call the FreeSWITCH API and return the response body verbatim.
    ///
    /// Transport errors and refused commands propagate as `EslError`; callers
    /// frame the latter through `EslError::command_failure`.
    async fn api_body(&self, client: &EslClient, command: &str) -> Result<String> {
        let response = client
            .api(command)
            .await?;
        match response.api_result() {
            Err(e)
                if e.command_failure()
                    .is_some() =>
            {
                Err(e.into())
            }
            // api_result() strips the "+OK " that a bare "+OK" reply is made
            // of entirely, and calls the empty rest a ProtocolError; that reply
            // is a success the display path still has to print.
            _ => Ok(response
                .body()
                .unwrap_or_default()
                .to_string()),
        }
    }

    /// Execute a FreeSWITCH command
    pub async fn execute_command(&self, client: &EslClient, command: &str) -> Result<()> {
        self.debug_level
            .debug_print(EslDebugLevel::Debug5, || {
                format!("execute_command called with: '{}'", command)
            });

        if let Some(result) = self
            .handle_special_command(client, command)
            .await?
        {
            self.print_message(&result);
            return Ok(());
        }

        match self
            .api_body(client, command)
            .await
        {
            Ok(body) => {
                if !body
                    .trim()
                    .is_empty()
                {
                    self.print_message(&body);
                }
            }
            Err(e) => {
                let esl = e.downcast_ref::<EslError>();
                if esl.is_some_and(EslError::is_connection_error) {
                    return Err(e);
                }
                let (label, text) = match esl.and_then(EslError::command_failure) {
                    Some(CommandFailure::Err(text) | CommandFailure::Unprefixed(text)) => {
                        ("API Error", text.to_string())
                    }
                    Some(CommandFailure::Usage(text)) => ("Usage", text.to_string()),
                    _ => ("API Error", format!("{:#}", e)),
                };
                self.print_error(&self.labeled_error(label, &text));
            }
        }

        Ok(())
    }

    /// Handle special CLI commands that need custom processing
    async fn handle_special_command(
        &self,
        client: &EslClient,
        command: &str,
    ) -> Result<Option<String>> {
        let parts: Vec<&str> = command
            .split_whitespace()
            .collect();
        if parts.is_empty() {
            self.debug_level
                .debug_print(EslDebugLevel::Debug6, || {
                    "handle_special_command: empty command".to_string()
                });
            return Ok(None);
        }

        self.debug_level
            .debug_print(EslDebugLevel::Debug5, || {
                format!("handle_special_command: parts[0] = '{}'", parts[0])
            });

        match parts[0]
            .to_lowercase()
            .as_str()
        {
            "/log" | "log" => {
                self.handle_log_command(client, &parts[1..])
                    .await
            }
            "uptime" => {
                let body = self
                    .api_body(client, "status")
                    .await?;
                Ok(Some(self.extract_uptime(&body)))
            }
            _ => Ok(None),
        }
    }

    /// Handle /log command with various log levels
    async fn handle_log_command(
        &self,
        client: &EslClient,
        parts: &[&str],
    ) -> Result<Option<String>> {
        if parts.is_empty() {
            return Ok(Some(LogSetting::help_text()));
        }

        let setting = match parts[0].parse::<LogSetting>() {
            Ok(setting) => setting,
            Err(message) => return Ok(Some(message)),
        };

        match set_log_level(client, setting).await? {
            None => Ok(Some(match setting {
                LogSetting::Level(level) => {
                    format!("+OK log level {} [{}]", level, level.as_number())
                }
                LogSetting::NoLog => "+OK log level nolog".to_string(),
            })),
            Some(reply) => Ok(Some(format!("Failed to set log level: {}", reply))),
        }
    }

    /// Extract uptime information from status output
    fn extract_uptime(&self, status_output: &str) -> String {
        for line in status_output.lines() {
            if line.contains("UP")
                && (line.contains("years") || line.contains("days") || line.contains("hours"))
            {
                return line
                    .trim()
                    .to_string();
            }
        }
        "Uptime information not found".to_string()
    }

    /// Show help information with the effective (merged) function key bindings.
    pub fn show_help(&self, macros: &HashMap<String, String>) {
        let mut fnkey_lines = String::new();
        for i in 1u8..=12 {
            let key = format!("f{}", i);
            if let Some(cmd) = macros.get(&key) {
                fnkey_lines.push_str(&format!("  F{:<3} = {}\n", i, cmd));
            }
        }

        let help_text = format!(
            r#"
FreeSWITCH CLI Commands:

Basic Commands:
  status                    - Show system status
  version                   - Show FreeSWITCH version
  uptime                    - Show system uptime

Show Commands:
  show channels             - List active channels
  show channels count       - Show channel count
  show calls                - Show active calls
  show registrations        - Show SIP registrations
  show modules              - List loaded modules
  show interfaces           - Show interfaces

Control Commands:
  reload [module]           - Reload module or XML config
  originate <url> <dest>    - Originate a call

Function Key Shortcuts (customizable in config):
{}
Built-in Commands:
  /help                     - Show this help
  /quit, /exit, /bye        - Exit the CLI
  /history                  - Show command history
  /clear                    - Clear screen

Configuration:
  Profiles can be configured in ~/.config/fs_cli.yaml or /etc/freeswitch/fs_cli.yaml
  Use --config to specify a custom configuration file path
  Use --list-profiles to see available profiles
  Default configuration is created automatically if missing

You can execute any FreeSWITCH API command directly.
Use Tab for command completion and Up/Down arrows for history.
"#,
            fnkey_lines
        );

        let formatted_help = if !self.no_color() {
            format!("{}", help_text.cyan())
        } else {
            help_text
        };
        self.print_message(&formatted_help);
    }
}
