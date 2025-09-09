//! Command processing and execution for fs_cli-rs

use crate::esl_debug::EslDebugLevel;
use anyhow::{Error, Result};
use colored::*;
use freeswitch_esl_rs::{command::EslCommand, EslHandle};
use rustyline::ExternalPrinter;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;

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
        match s.to_lowercase().as_str() {
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogLevel {
    Console,
    Alert,
    Crit,
    Err,
    Warning,
    Notice,
    Info,
    Debug,
    Debug1,
    Debug2,
    Debug3,
    Debug4,
    Debug5,
    Debug6,
    Debug7,
    Debug8,
    Debug9,
    Debug10,
    NoLog,
}

impl FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s.to_lowercase().as_str() {
            "console" => Ok(LogLevel::Console),
            "alert" => Ok(LogLevel::Alert),
            "crit" => Ok(LogLevel::Crit),
            "err" | "error" => Ok(LogLevel::Err),
            "warning" | "warn" => Ok(LogLevel::Warning),
            "notice" => Ok(LogLevel::Notice),
            "info" => Ok(LogLevel::Info),
            "debug" => Ok(LogLevel::Debug),
            "debug1" => Ok(LogLevel::Debug1),
            "debug2" => Ok(LogLevel::Debug2),
            "debug3" => Ok(LogLevel::Debug3),
            "debug4" => Ok(LogLevel::Debug4),
            "debug5" => Ok(LogLevel::Debug5),
            "debug6" => Ok(LogLevel::Debug6),
            "debug7" => Ok(LogLevel::Debug7),
            "debug8" => Ok(LogLevel::Debug8),
            "debug9" => Ok(LogLevel::Debug9),
            "debug10" => Ok(LogLevel::Debug10),
            "nolog" => Ok(LogLevel::NoLog),
            _ => Err(format!("Invalid log level: {}", s)),
        }
    }
}

impl LogLevel {
    /// Convert log level to the level string for FreeSWITCH command
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Console => "console",
            LogLevel::Alert => "alert",
            LogLevel::Crit => "crit",
            LogLevel::Err => "err",
            LogLevel::Warning => "warning",
            LogLevel::Notice => "notice",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Debug1 => "debug1",
            LogLevel::Debug2 => "debug2",
            LogLevel::Debug3 => "debug3",
            LogLevel::Debug4 => "debug4",
            LogLevel::Debug5 => "debug5",
            LogLevel::Debug6 => "debug6",
            LogLevel::Debug7 => "debug7",
            LogLevel::Debug8 => "debug8",
            LogLevel::Debug9 => "debug9",
            LogLevel::Debug10 => "debug10",
            LogLevel::NoLog => "nolog",
        }
    }

    /// Get all available log levels for help text
    pub fn all_variants() -> &'static [LogLevel] {
        &[
            LogLevel::Console,
            LogLevel::Alert,
            LogLevel::Crit,
            LogLevel::Err,
            LogLevel::Warning,
            LogLevel::Notice,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Debug1,
            LogLevel::Debug2,
            LogLevel::Debug3,
            LogLevel::Debug4,
            LogLevel::Debug5,
            LogLevel::Debug6,
            LogLevel::Debug7,
            LogLevel::Debug8,
            LogLevel::Debug9,
            LogLevel::Debug10,
            LogLevel::NoLog,
        ]
    }

    /// Get help text with all available levels
    pub fn help_text() -> String {
        let levels: Vec<&str> = Self::all_variants().iter().map(|l| l.as_str()).collect();
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
    printer: Option<Arc<Mutex<dyn ExternalPrinter + Send>>>,
}

impl CommandProcessor {
    /// Create new command processor
    pub fn new(color_mode: ColorMode, debug_level: EslDebugLevel) -> Self {
        Self {
            color_mode,
            debug_level,
            printer: None,
        }
    }

    /// Check if colors should be disabled
    fn no_color(&self) -> bool {
        self.color_mode == ColorMode::Never
    }

    /// Set external printer for coordinated output
    pub fn set_printer(&mut self, printer: Option<Arc<Mutex<dyn ExternalPrinter + Send>>>) {
        self.printer = printer;
    }

    /// Print message using external printer or fallback to println
    async fn print_message(&self, message: &str) {
        if let Some(printer_arc) = &self.printer {
            if let Ok(mut p) = printer_arc.try_lock() {
                let _ = p.print(message.to_string());
            } else {
                // Fallback if printer is locked
                println!("{}", message);
            }
        } else {
            println!("{}", message);
        }
    }

    /// Print error message using external printer or fallback to eprintln
    async fn print_error(&self, message: &str) {
        if let Some(printer_arc) = &self.printer {
            if let Ok(mut p) = printer_arc.try_lock() {
                let _ = p.print(message.to_string());
            } else {
                // Fallback if printer is locked
                eprintln!("{}", message);
            }
        } else {
            eprintln!("{}", message);
        }
    }

    /// Handle command execution errors with proper formatting
    pub async fn handle_error(&self, error: Error) {
        let error_msg = if !self.no_color() {
            format!("{}: {}", "Error".red().bold(), error)
        } else {
            format!("Error: {}", error)
        };
        self.print_error(&error_msg).await;
    }

    /// Execute a FreeSWITCH command
    pub async fn execute_command(&self, handle: &mut EslHandle, command: &str) -> Result<()> {
        self.debug_level.debug_print(
            EslDebugLevel::Debug5,
            &format!("execute_command called with: '{}'", command),
        );

        // Handle special commands
        if let Some(result) = self.handle_special_command(handle, command).await? {
            self.print_message(&result).await;
            return Ok(());
        }

        // Execute as API command
        match handle.api(command).await {
            Ok(response) => {
                if !response.is_success() {
                    if let Some(reply) = response.reply_text() {
                        let error_msg = if !self.no_color() {
                            format!("{}: {}", "API Error".red().bold(), reply)
                        } else {
                            format!("API Error: {}", reply)
                        };
                        self.print_error(&error_msg).await;
                        return Ok(()); // Don't treat API errors as fatal
                    }
                }

                let body = response.body_string();
                if !body.trim().is_empty() {
                    self.print_message(&body).await;
                }
            }
            Err(e) => {
                return Err(e.into());
            }
        }

        Ok(())
    }

    /// Handle special CLI commands that need custom processing
    async fn handle_special_command(
        &self,
        handle: &mut EslHandle,
        command: &str,
    ) -> Result<Option<String>> {
        let parts: Vec<&str> = command.split_whitespace().collect();
        if parts.is_empty() {
            self.debug_level.debug_print(
                EslDebugLevel::Debug6,
                "handle_special_command: empty command",
            );
            return Ok(None);
        }

        self.debug_level.debug_print(
            EslDebugLevel::Debug5,
            &format!("handle_special_command: parts[0] = '{}'", parts[0]),
        );

        match parts[0] {
            "/log" => {
                self.debug_level
                    .debug_print(EslDebugLevel::Debug6, "Matched /log command");
                self.handle_log_command(handle, &parts[1..]).await
            }
            _ => match parts[0].to_lowercase().as_str() {
                "show" if parts.len() > 1 => self.handle_show_command(handle, &parts[1..]).await,
                "status" => {
                    let response = handle.api("status").await?;
                    Ok(Some(response.body_string()))
                }
                "version" => {
                    let response = handle.api("version").await?;
                    Ok(Some(response.body_string()))
                }
                "uptime" => {
                    let response = handle.api("status").await?;
                    Ok(Some(self.extract_uptime(&response.body_string())))
                }
                "reload" => {
                    if parts.len() > 1 {
                        let module = parts[1];
                        let response = handle.api(&format!("reload {}", module)).await?;
                        Ok(Some(format!(
                            "Reloaded module: {}\n{}",
                            module,
                            response.body_string()
                        )))
                    } else {
                        let response = handle.api("reloadxml").await?;
                        Ok(Some(format!(
                            "Reloaded XML configuration\n{}",
                            response.body_string()
                        )))
                    }
                }
                "originate" => {
                    if parts.len() >= 3 {
                        let call_string = parts[1..].join(" ");
                        let response = handle.api(&format!("originate {}", call_string)).await?;
                        Ok(Some(format!(
                            "Originate command executed\n{}",
                            response.body_string()
                        )))
                    } else {
                        Ok(Some(
                            "Usage: originate <call_url> <destination>".to_string(),
                        ))
                    }
                }
                _ => Ok(None), // Not a special command
            },
        }
    }

    /// Handle /log command with various log levels
    async fn handle_log_command(
        &self,
        handle: &mut EslHandle,
        parts: &[&str],
    ) -> Result<Option<String>> {
        if parts.is_empty() {
            return Ok(Some(LogLevel::help_text()));
        }

        let log_level = match parts[0].parse::<LogLevel>() {
            Ok(level) => level,
            Err(err) => return Ok(Some(err)),
        };

        // Send the log command directly to FreeSWITCH (not as API)
        let cmd = if log_level == LogLevel::NoLog {
            EslCommand::Api {
                command: "nolog".to_string(),
            }
        } else {
            EslCommand::Log {
                level: log_level.as_str().to_string(),
            }
        };
        let response = handle.send_command(cmd).await?;

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
                    .map(|s| s.as_str())
                    .unwrap_or("Unknown error")
            )))
        }
    }

    /// Handle 'show' commands with enhanced formatting
    async fn handle_show_command(
        &self,
        handle: &mut EslHandle,
        parts: &[&str],
    ) -> Result<Option<String>> {
        if parts.is_empty() {
            return Ok(Some(
                "Usage: show <channels|calls|registrations|modules|...>".to_string(),
            ));
        }

        let subcommand = parts[0].to_lowercase();
        let command = match subcommand.as_str() {
            "channels" => {
                if parts.len() > 1 && parts[1] == "count" {
                    "show channels count"
                } else {
                    "show channels"
                }
            }
            "calls" => "show calls",
            "registrations" => "sofia status",
            "modules" => "show modules",
            "interfaces" => "show interfaces",
            "api" => "show api",
            "application" => "show application",
            "codec" => "show codec",
            "file" => "show file",
            "timer" => "show timer",
            "tasks" => "show tasks",
            "complete" => "show complete",
            _ => {
                return Ok(Some(format!("Unknown show command: {}\n\
                Available: channels, calls, registrations, modules, interfaces, api, application, codec, file, timer, tasks",
                subcommand)));
            }
        };

        let response = handle.api(command).await?;
        Ok(Some(response.body_string()))
    }

    /// Extract uptime information from status output
    fn extract_uptime(&self, status_output: &str) -> String {
        for line in status_output.lines() {
            if line.contains("UP")
                && (line.contains("years") || line.contains("days") || line.contains("hours"))
            {
                return line.trim().to_string();
            }
        }
        "Uptime information not found".to_string()
    }

    /// Show help information
    pub async fn show_help(&self) {
        let help_text = r#"
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

Function Key Shortcuts:
  F1  = help                F7  = /log console
  F2  = status              F8  = /log debug
  F3  = show channels       F9  = sofia status profile internal
  F4  = show calls          F10 = fsctl pause
  F5  = sofia status        F11 = fsctl resume
  F6  = reloadxml           F12 = version

Built-in Commands:
  /help                     - Show this help
  /quit, /exit, /bye        - Exit the CLI
  history                   - Show command history
  clear                     - Clear screen

You can execute any FreeSWITCH API command directly.
Use Tab for command completion and Up/Down arrows for history.
"#;

        let formatted_help = if !self.no_color() {
            format!("{}", help_text.cyan())
        } else {
            help_text.to_string()
        };
        self.print_message(&formatted_help).await;
    }
}
