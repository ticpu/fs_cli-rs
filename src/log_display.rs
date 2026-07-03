//! Log display functionality for fs_cli-rs

use crate::commands::ColorMode;
use crate::printer::Printer;
use colored::*;
use freeswitch_esl_tokio::{EslEvent, EventHeader};

/// Log display helper functions
pub struct LogDisplay;

impl LogDisplay {
    pub fn is_log_event(event: &EslEvent) -> bool {
        event
            .header_str("Content-Type")
            .is_some_and(|ct| ct.eq_ignore_ascii_case("log/data"))
    }

    /// Display a log event with appropriate formatting and colors.
    pub fn display_log_event(event: &EslEvent, color_mode: ColorMode, printer: &Printer) {
        let log_level = event
            .header(EventHeader::LogLevel)
            .and_then(|raw| {
                raw.parse::<u32>()
                    .ok()
                    .or_else(|| {
                        tracing::debug!("unparseable Log-Level {:?}, defaulting to 7", raw);
                        None
                    })
            })
            .unwrap_or(7);

        let message = event
            .body()
            .unwrap_or("");
        if message
            .trim()
            .is_empty()
        {
            return;
        }

        let formatted_message = match color_mode {
            ColorMode::Never => message
                .trim()
                .to_string(),
            ColorMode::Tag => Self::format_colored_log_tag_only(message.trim(), log_level),
            ColorMode::Line => Self::format_colored_log_full_line(message.trim(), log_level),
        };

        printer.print(formatted_message);
    }

    /// Apply color based on log level
    fn colorize_by_level(text: &str, log_level: u32) -> ColoredString {
        match log_level {
            0 => text
                .white()
                .bold(), // CONSOLE
            1 | 2 => text
                .red()
                .bold(), // ALERT / CRIT
            3 => text.red(),    // ERR
            4 => text.yellow(), // WARNING
            5 => text.cyan(),   // NOTICE
            6 => text.green(),  // INFO - green like real fs_cli
            _ => text
                .yellow()
                .dimmed(), // DEBUG and higher
        }
    }

    /// Format colored log message with only tag colorized
    fn format_colored_log_tag_only(message: &str, log_level: u32) -> String {
        if let Some(level_start) = message.find('[') {
            if let Some(level_end) = message[level_start..].find(']') {
                let level_end = level_start + level_end + 1;
                let before = &message[..level_start];
                let level_tag = &message[level_start..level_end];
                let after = &message[level_end..];

                let colored_level = Self::colorize_by_level(level_tag, log_level);

                return format!("{}{}{}", before, colored_level, after);
            }
        }

        message.to_string()
    }

    /// Format colored log message with entire line colorized
    fn format_colored_log_full_line(message: &str, log_level: u32) -> String {
        Self::colorize_by_level(message, log_level).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use freeswitch_esl_tokio::EslEvent;

    #[test]
    fn is_log_event_with_log_data_content_type() {
        let mut event = EslEvent::new();
        event.set_header("Content-Type", "log/data");
        event.set_header("Log-Level", "6");
        assert!(LogDisplay::is_log_event(&event));
    }

    #[test]
    fn is_log_event_rejects_normal_event() {
        let mut event = EslEvent::new();
        event.set_header("Event-Name", "CHANNEL_CREATE");
        assert!(!LogDisplay::is_log_event(&event));
    }

    #[test]
    fn is_log_event_rejects_empty_event() {
        let event = EslEvent::new();
        assert!(!LogDisplay::is_log_event(&event));
    }
}
