//! Command processing and execution for fs_cli-rs

use crate::client_command::{ClientCommand, ParseError};
use crate::log_level::{set_log_level, LogSetting};
use crate::printer::Output;
use anyhow::{Error, Result};
use colored::*;
use freeswitch_esl_tokio::{CommandFailure, EslClient, EslError};
use std::collections::HashMap;
use tracing::trace;

/// Command processor for FreeSWITCH CLI commands
pub struct CommandProcessor {
    output: Output,
}

impl CommandProcessor {
    /// Create new command processor
    pub fn new(output: &Output) -> Self {
        Self {
            output: output.clone(),
        }
    }

    fn print_message(&self, message: &str) {
        self.output
            .print(message.to_string());
    }

    /// Handle command execution errors with proper formatting
    pub fn handle_error(&self, error: Error) {
        self.output
            .print_labeled_error("Error", &error);
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
        trace!("execute_command called with: '{}'", command);

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
                match esl.and_then(EslError::command_failure) {
                    Some(CommandFailure::Err(text) | CommandFailure::Unprefixed(text)) => self
                        .output
                        .print_labeled("API Error", text),
                    Some(CommandFailure::Usage(text)) => self
                        .output
                        .print_labeled("Usage", text),
                    _ => self
                        .output
                        .print_labeled_error("API Error", &e),
                }
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
            trace!("handle_special_command: empty command");
            return Ok(None);
        }

        trace!("handle_special_command: parts[0] = '{}'", parts[0]);

        match command.parse::<ClientCommand>() {
            Ok(ClientCommand::Log(level)) => {
                return self
                    .handle_log_command(client, level)
                    .await
            }
            Err(ParseError::InvalidLogLevel(level)) => {
                return Ok(Some(format!("Invalid log level: {}", level)))
            }
            _ => {}
        }

        if parts[0].eq_ignore_ascii_case("uptime") {
            let body = self
                .api_body(client, "status")
                .await?;
            return Ok(Some(self.extract_uptime(&body)));
        }

        Ok(None)
    }

    /// Handle /log command, with no level meaning "list the levels"
    pub async fn handle_log_command(
        &self,
        client: &EslClient,
        level: Option<LogSetting>,
    ) -> Result<Option<String>> {
        let Some(setting) = level else {
            return Ok(Some(LogSetting::help_text()));
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
        for (i, cmd) in crate::readline::fn_key_bindings(macros) {
            fnkey_lines.push_str(&format!("  F{:<3} = {}\n", i, cmd));
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

        let formatted_help = self
            .output
            .colorize(&help_text, |s| s.cyan());
        self.print_message(&formatted_help);
    }
}
