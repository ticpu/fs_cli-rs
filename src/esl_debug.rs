//! ESL client-side debug logging functionality
//!
//! Implements debug levels similar to the original fs_cli -d option (0-7)
//! for controlling ESL protocol message logging on the client side.

use serde::{Deserialize, Serialize};
use strum::FromRepr;

/// ESL client-side debug levels (0-7)
/// Matches the original fs_cli esl_global_set_default_logger levels
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default, FromRepr, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
#[repr(u8)]
pub enum EslDebugLevel {
    #[default]
    None = 0,
    Error = 1,
    Warning = 2,
    Info = 3,
    Debug = 4,
    Debug5 = 5,
    Debug6 = 6,
    Debug7 = 7,
}

impl TryFrom<u8> for EslDebugLevel {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> anyhow::Result<Self> {
        Self::from_repr(value)
            .ok_or_else(|| anyhow::anyhow!("Invalid ESL debug level: {} (must be 0-7)", value))
    }
}

impl From<EslDebugLevel> for u8 {
    fn from(level: EslDebugLevel) -> Self {
        level as Self
    }
}

impl EslDebugLevel {
    /// Get tracing filter level for this debug level
    pub fn tracing_filter(&self) -> &'static str {
        match self {
            EslDebugLevel::None => "error",
            EslDebugLevel::Error => "error",
            EslDebugLevel::Warning => "warn",
            EslDebugLevel::Info => "info",
            EslDebugLevel::Debug | EslDebugLevel::Debug5 => {
                "fs_cli_rs=debug,freeswitch_esl_tokio=debug,rustyline=warn"
            }
            EslDebugLevel::Debug6 | EslDebugLevel::Debug7 => {
                "fs_cli_rs=trace,freeswitch_esl_tokio=trace,rustyline=warn"
            }
        }
    }
}
