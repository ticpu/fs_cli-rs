//! Log display functionality for fs_cli-rs

use crate::printer::{ColorMode, Output};
use colored::{ColoredString, Colorize};
use freeswitch_esl_tokio::{EslEvent, EslEventType, EventHeader, HeaderLookup};
use tracing::debug;

pub fn is_log_event(event: &EslEvent) -> bool {
    event
        .header_str("Content-Type")
        .is_some_and(|ct| ct.eq_ignore_ascii_case("log/data"))
}

/// FreeSWITCH's numeric DEBUG level, assumed when Log-Level is missing or unparseable.
const DEFAULT_LOG_LEVEL: u32 = 7;

/// Display a log event with appropriate formatting and colors.
pub fn display_log_event(event: &EslEvent, output: &Output) {
    let log_level = event
        .header(EventHeader::LogLevel)
        .and_then(|raw| {
            raw.parse::<u32>()
                .ok()
                .or_else(|| {
                    debug!(
                        "unparseable Log-Level {:?}, defaulting to {}",
                        raw, DEFAULT_LOG_LEVEL
                    );
                    None
                })
        })
        .unwrap_or(DEFAULT_LOG_LEVEL);

    let message = event
        .body()
        .unwrap_or("");
    if message
        .trim()
        .is_empty()
    {
        return;
    }

    let formatted_message = match output.color() {
        ColorMode::Never => message
            .trim()
            .to_string(),
        ColorMode::Tag => format_colored_log_tag_only(message.trim(), log_level),
        ColorMode::Line => format_colored_log_full_line(message.trim(), log_level),
    };

    output.print(formatted_message);
}

/// `<number> name` for a channel, or None when neither is set.
pub fn format_caller_id(number: &str, name: &str) -> Option<String> {
    (!number.is_empty() || !name.is_empty()).then(|| format!("<{}> {}", number, name))
}

/// One line for a channel lifecycle event, or None for anything else.
pub fn format_channel_event(event: &EslEvent, output: &Output) -> Option<String> {
    let event_type = event.event_type()?;

    let label = match event_type {
        EslEventType::ChannelCreate => "CREATE",
        EslEventType::ChannelAnswer => "ANSWER",
        EslEventType::ChannelHangup => "HANGUP",
        _ => return None,
    };

    let channel = event
        .channel_name()
        .unwrap_or("unknown");
    let uuid = event
        .unique_id()
        .unwrap_or("?");

    let line = if event_type == EslEventType::ChannelHangup {
        let cause = match event.hangup_cause() {
            Ok(Some(c)) => c.to_string(),
            Ok(None) => "unknown".to_string(),
            Err(e) => e.to_string(),
        };
        format!("[{}] {} {} ({})", label, uuid, channel, cause)
    } else {
        let caller_id = format_caller_id(
            event
                .caller_id_number()
                .unwrap_or(""),
            event
                .caller_id_name()
                .unwrap_or(""),
        );
        match caller_id {
            Some(cid) => format!("[{}] {} {} {}", label, uuid, channel, cid),
            None => format!("[{}] {} {}", label, uuid, channel),
        }
    };

    Some(output.colorize(&line, |s| s.cyan()))
}

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
        6 => text.green(),  // INFO
        _ => text
            .yellow()
            .dimmed(), // DEBUG and higher
    }
}

fn format_colored_log_tag_only(message: &str, log_level: u32) -> String {
    if let Some(level_start) = message.find('[') {
        if let Some(level_end) = message[level_start..].find(']') {
            let level_end = level_start + level_end + 1;
            let before = &message[..level_start];
            let level_tag = &message[level_start..level_end];
            let after = &message[level_end..];

            let colored_level = colorize_by_level(level_tag, log_level);

            return format!("{}{}{}", before, colored_level, after);
        }
    }

    message.to_string()
}

fn format_colored_log_full_line(message: &str, log_level: u32) -> String {
    colorize_by_level(message, log_level).to_string()
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
        assert!(is_log_event(&event));
    }

    #[test]
    fn is_log_event_rejects_non_log_events() {
        let mut event = EslEvent::new();
        event.set_header("Event-Name", "CHANNEL_CREATE");
        assert!(!is_log_event(&event));

        let empty_event = EslEvent::new();
        assert!(!is_log_event(&empty_event));
    }

    #[test]
    fn format_colored_log_tag_only_with_tag() {
        let message = "2024 [NOTICE] something happened";
        let formatted = format_colored_log_tag_only(message, 5);
        assert!(formatted.contains("2024 "));
        assert!(formatted.contains("[NOTICE]"));
        assert!(formatted.contains(" something happened"));
    }

    #[test]
    fn format_colored_log_tag_only_without_tag() {
        let message = "no bracket tag here";
        assert_eq!(format_colored_log_tag_only(message, 5), message);
    }
}
