//! Command processing and execution for fs_cli-rs

use crate::esl_debug::EslDebugLevel;
use crate::printer::Printer;
use anyhow::{anyhow, Error, Result};
use colored::*;
use freeswitch_esl_tokio::{EslClient, EslError};
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

/// FreeSWITCH log levels
#[derive(
    Debug, Clone, Copy, PartialEq, strum::EnumString, strum::IntoStaticStr, strum::EnumIter,
)]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
#[repr(u8)]
pub enum LogLevel {
    Console = 0,
    Alert = 1,
    Crit = 2,
    #[strum(serialize = "error")]
    Err = 3,
    #[strum(serialize = "warn")]
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
    Debug1 = 8,
    Debug2 = 9,
    Debug3 = 10,
    Debug4 = 11,
    Debug5 = 12,
    Debug6 = 13,
    Debug7 = 14,
    Debug8 = 15,
    Debug9 = 16,
    Debug10 = 17,
    NoLog = 18,
}

impl LogLevel {
    /// Convert log level to the level string for FreeSWITCH command
    pub fn as_str(&self) -> &'static str {
        self.into()
    }

    /// Get all available log levels for help text
    pub fn all_variants() -> &'static [LogLevel] {
        use strum::IntoEnumIterator;
        static VARIANTS: std::sync::OnceLock<Vec<LogLevel>> = std::sync::OnceLock::new();
        VARIANTS.get_or_init(|| LogLevel::iter().collect())
    }

    /// Get help text with all available levels
    pub fn help_text() -> String {
        let levels: Vec<&str> = Self::all_variants()
            .iter()
            .map(|l| l.as_str())
            .collect();
        format!(
            "Usage: /log <level>\nAvailable levels: {}",
            levels.join(", ")
        )
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

    /// Handle command execution errors with proper formatting
    pub fn handle_error(&self, error: Error) {
        let error_msg = if !self.no_color() {
            format!(
                "{}: {}",
                "Error"
                    .red()
                    .bold(),
                error
            )
        } else {
            format!("Error: {}", error)
        };
        self.print_error(&error_msg);
    }

    /// Call the FreeSWITCH API, check success, and return the response body.
    ///
    /// Transport errors propagate as EslError. A non-success API response
    /// returns an Err carrying the reply text (without "API Error:" prefix —
    /// callers add their own framing).
    async fn api_body(&self, client: &EslClient, command: &str) -> Result<String> {
        let response = client
            .api(command)
            .await?;
        if !response.is_success() {
            let reply = response
                .reply_text()
                .unwrap_or("unknown error");
            return Err(anyhow!("{}", reply));
        }
        Ok(response
            .body()
            .unwrap_or_default()
            .to_string())
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
            Err(e)
                if e.downcast_ref::<EslError>()
                    .is_some() =>
            {
                return Err(e);
            }
            Err(e) => {
                let error_msg = if !self.no_color() {
                    format!(
                        "{}: {}",
                        "API Error"
                            .red()
                            .bold(),
                        e
                    )
                } else {
                    format!("API Error: {}", e)
                };
                self.print_error(&error_msg);
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

        match parts[0] {
            "/log" => {
                self.debug_level
                    .debug_print(EslDebugLevel::Debug6, || "Matched /log command".to_string());
                self.handle_log_command(client, &parts[1..])
                    .await
            }
            _ => match parts[0]
                .to_lowercase()
                .as_str()
            {
                "log" => {
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
            },
        }
    }

    /// Handle /log command with various log levels
    async fn handle_log_command(
        &self,
        client: &EslClient,
        parts: &[&str],
    ) -> Result<Option<String>> {
        if parts.is_empty() {
            return Ok(Some(LogLevel::help_text()));
        }

        let log_level = match parts[0].parse::<LogLevel>() {
            Ok(level) => level,
            Err(_) => {
                return Ok(Some(format!("Invalid log level: {}", parts[0])));
            }
        };

        let response = if log_level == LogLevel::NoLog {
            client
                .nolog()
                .await?
        } else {
            client
                .log(log_level.as_str())
                .await?
        };

        if response.is_success() {
            Ok(Some(format!(
                "+OK log level {} [{}]",
                log_level.as_str(),
                log_level as u8
            )))
        } else {
            Ok(Some(format!(
                "Failed to set log level: {}",
                response
                    .reply_text()
                    .unwrap_or("Unknown error")
            )))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_level_parse_all_valid() {
        for level in LogLevel::all_variants() {
            let parsed: Result<LogLevel, _> = level
                .as_str()
                .parse();
            assert!(parsed.is_ok(), "failed to parse '{}'", level.as_str());
            assert_eq!(parsed.unwrap(), *level);
        }
    }

    #[test]
    fn log_level_parse_invalid_returns_error() {
        let result: Result<LogLevel, _> = "hello".parse();
        assert!(result.is_err());
    }

    #[test]
    fn log_level_parse_case_insensitive() {
        let result: Result<LogLevel, _> = "DEBUG".parse();
        assert_eq!(result.unwrap(), LogLevel::Debug);
    }

    #[test]
    fn log_level_parse_aliases() {
        let err: Result<LogLevel, _> = "error".parse();
        assert_eq!(err.unwrap(), LogLevel::Err);
        let warn: Result<LogLevel, _> = "warn".parse();
        assert_eq!(warn.unwrap(), LogLevel::Warning);
    }

    #[test]
    fn log_level_as_u8_discriminants() {
        assert_eq!(LogLevel::Console as u8, 0);
        assert_eq!(LogLevel::Alert as u8, 1);
        assert_eq!(LogLevel::Crit as u8, 2);
        assert_eq!(LogLevel::Err as u8, 3);
        assert_eq!(LogLevel::Warning as u8, 4);
        assert_eq!(LogLevel::Debug as u8, 7);
        assert_eq!(LogLevel::NoLog as u8, 18);
    }
}
