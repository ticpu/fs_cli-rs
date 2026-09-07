//! The `/log` argument: a FreeSWITCH log level, or the `nolog` off switch.

use anyhow::Result;
use freeswitch_esl_tokio::{EslClient, LogLevel};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogSetting {
    Level(LogLevel),
    NoLog,
}

impl LogSetting {
    /// Wire names `/log` accepts: every `LogLevel` but `Disable`, plus `nolog`.
    pub(crate) fn level_names() -> Vec<&'static str> {
        let mut levels: Vec<&str> = LogLevel::ALL
            .iter()
            .filter(|l| **l != LogLevel::Disable)
            .map(|l| l.as_str())
            .collect();
        levels.push("nolog");
        levels
    }

    /// Help text listing every level `/log` accepts.
    pub(crate) fn help_text() -> String {
        format!(
            "Usage: /log <level>\nAvailable levels: {}",
            Self::level_names().join(", ")
        )
    }
}

impl FromStr for LogSetting {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        let reject = || format!("Invalid log level: {}", s);
        // LogLevel::FromStr is an exact match on the lowercase wire name.
        let lowered = s.to_lowercase();

        if lowered == "nolog" {
            return Ok(LogSetting::NoLog);
        }
        if let Ok(n) = lowered.parse::<i8>() {
            return (0..=7)
                .contains(&n)
                .then(|| LogLevel::from_number(n))
                .flatten()
                .map(LogSetting::Level)
                .ok_or_else(reject);
        }
        let level = match lowered.as_str() {
            "error" => LogLevel::Error,
            "warn" => LogLevel::Warning,
            // `nolog` is the one off switch; Disable is not offered.
            "disable" => return Err(reject()),
            name => LogLevel::from_str(name).map_err(|_| reject())?,
        };
        Ok(LogSetting::Level(level))
    }
}

impl std::fmt::Display for LogSetting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogSetting::Level(level) => f.write_str(level.as_str()),
            LogSetting::NoLog => f.write_str("nolog"),
        }
    }
}

impl Serialize for LogSetting {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for LogSetting {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Send a log-level command to FreeSWITCH.
///
/// Returns `Ok(None)` on server success, `Ok(Some(reply))` when the server
/// rejects the request, `Err` on transport failure.
pub(crate) async fn set_log_level(
    client: &EslClient,
    setting: LogSetting,
) -> Result<Option<String>> {
    let response = match setting {
        LogSetting::NoLog => {
            client
                .nolog()
                .await?
        }
        LogSetting::Level(level) => {
            client
                .log(level)
                .await?
        }
    };
    if response.is_success() {
        Ok(None)
    } else {
        Ok(Some(
            response
                .reply_text()
                .unwrap_or("Unknown error")
                .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> std::result::Result<LogSetting, String> {
        s.parse()
    }

    #[test]
    fn parses_aliases_names_numbers_and_nolog() {
        assert_eq!(parse("error"), Ok(LogSetting::Level(LogLevel::Error)));
        assert_eq!(parse("warn"), Ok(LogSetting::Level(LogLevel::Warning)));
        assert_eq!(parse("7"), Ok(LogSetting::Level(LogLevel::Debug)));
        assert_eq!(parse("0"), Ok(LogSetting::Level(LogLevel::Console)));
        assert_eq!(parse("nolog"), Ok(LogSetting::NoLog));
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(parse("DEBUG"), Ok(LogSetting::Level(LogLevel::Debug)));
        assert_eq!(parse("NoLog"), Ok(LogSetting::NoLog));
        assert_eq!(parse("Warn"), Ok(LogSetting::Level(LogLevel::Warning)));
    }

    #[test]
    fn renders_the_wire_name_switch_log_str2level_knows() {
        assert_eq!(LogSetting::Level(LogLevel::Error).to_string(), "err");
        assert_eq!(LogSetting::Level(LogLevel::Warning).to_string(), "warning");
        assert_eq!(LogSetting::NoLog.to_string(), "nolog");
    }

    #[test]
    fn rejects_disable_out_of_range_and_unknown() {
        assert!(parse("-1").is_err());
        assert!(parse("disable").is_err());
        assert!(parse("8").is_err());
        assert!(parse("debug1").is_err());
        assert!(parse("hello").is_err());
    }

    #[test]
    fn help_text_offers_nolog_and_hides_disable() {
        let help = LogSetting::help_text();
        assert!(help.contains("nolog"));
        assert!(!help.contains("disable"));
        assert!(help.contains("err"));
    }
}
