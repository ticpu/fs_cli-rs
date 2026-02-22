//! fs_cli-rs: Interactive FreeSWITCH CLI client using ESL

use anyhow::{Context, Result};
use freeswitch_esl_tokio::{EslClient, EslError, EslEventStream, EslEventType, EventFormat};
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

mod args;
mod channel_info;
mod commands;
mod completion;
mod config;
mod console_complete;
mod esl_debug;
mod log_display;
mod readline;
mod session;

use args::Args;
use commands::CommandProcessor;
use config::AppConfig;
use esl_debug::EslDebugLevel;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let config = Args::parse_and_merge()?;

    crate::esl_debug::init_global_debug_level(config.debug);
    setup_logging(config.debug)?;

    config
        .debug
        .debug_print(EslDebugLevel::Debug, "About to connect to FreeSWITCH");
    let (client, events) = match connect_to_freeswitch_with_retry(&config).await {
        Ok(pair) => {
            config
                .debug
                .debug_print(EslDebugLevel::Debug, "Successfully connected to FreeSWITCH");
            pair
        }
        Err(e) => {
            print_connect_error(&e, &config);
            std::process::exit(1);
        }
    };

    if !config
        .execute
        .is_empty()
    {
        execute_commands(&client, &config.execute, &config).await?;
        info!("Disconnecting from FreeSWITCH...");
        client
            .disconnect()
            .await?;
    } else {
        if config.events {
            config
                .debug
                .debug_print(EslDebugLevel::Debug, "Subscribing to events");
            subscribe_to_events(&client).await?;
        }

        if !config.quiet {
            config
                .debug
                .debug_print(
                    EslDebugLevel::Debug,
                    &format!(
                        "Enabling logging at level: {}",
                        config
                            .log_level
                            .as_str()
                    ),
                );
            enable_logging(&client, config.log_level).await?;
        }

        if let Err(e) = session::run_interactive_mode(client, events, &config).await {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

fn setup_logging(debug_level: EslDebugLevel) -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(debug_level.tracing_filter())
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();
    Ok(())
}

/// Connect to FreeSWITCH with timeout
pub async fn connect_to_freeswitch(config: &AppConfig) -> Result<(EslClient, EslEventStream)> {
    info!(
        "Connecting to FreeSWITCH at {}:{}",
        config.host, config.port
    );

    let result = if let Some(ref user) = config.user {
        info!("Using user authentication: {}", user);
        timeout(
            Duration::from_millis(config.timeout),
            EslClient::connect_with_user(&config.host, config.port, user, &config.password),
        )
        .await
    } else {
        info!("Using password authentication");
        timeout(
            Duration::from_millis(config.timeout),
            EslClient::connect(&config.host, config.port, &config.password),
        )
        .await
    };

    let (client, events) = result
        .context("Connection timed out")?
        .context("Failed to connect to FreeSWITCH")?;

    Ok((client, events))
}

async fn connect_to_freeswitch_with_retry(
    config: &AppConfig,
) -> Result<(EslClient, EslEventStream)> {
    if !config.retry {
        return connect_to_freeswitch(config).await;
    }

    info!(
        "Retry mode enabled - will retry every {} ms",
        config.timeout
    );

    loop {
        match connect_to_freeswitch(config).await {
            Ok(pair) => return Ok(pair),
            Err(e) => {
                warn!("Connection attempt failed: {}", e);
                info!("Retrying in {} ms...", config.timeout);
                tokio::time::sleep(Duration::from_millis(config.timeout)).await;
            }
        }
    }
}

/// Check if error indicates connection loss
pub fn is_connection_error(error: &anyhow::Error) -> bool {
    if let Some(esl_err) = error.downcast_ref::<EslError>() {
        return esl_err.is_connection_error();
    }
    if let Some(io_err) = error.downcast_ref::<std::io::Error>() {
        return matches!(
            io_err.kind(),
            std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::UnexpectedEof
        );
    }
    false
}

/// Subscribe to events for monitoring
pub async fn subscribe_to_events(client: &EslClient) -> Result<()> {
    info!("Subscribing to events...");
    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::ChannelCreate,
                EslEventType::ChannelAnswer,
                EslEventType::ChannelHangup,
                EslEventType::Heartbeat,
            ],
        )
        .await?;
    println!("Event monitoring enabled");
    Ok(())
}

/// Enable logging at the specified level
pub async fn enable_logging(
    client: &EslClient,
    log_level: crate::commands::LogLevel,
) -> Result<()> {
    info!("Enabling logging at level: {}", log_level.as_str());

    let response = if log_level == crate::commands::LogLevel::NoLog {
        client.nolog().await?
    } else {
        client
            .log(log_level.as_str())
            .await?
    };
    if !response.is_success() {
        if let Some(reply) = response.reply_text() {
            warn!("Failed to set log level: {}", reply);
        }
    }
    Ok(())
}

async fn execute_commands(
    client: &EslClient,
    commands: &[String],
    config: &AppConfig,
) -> Result<()> {
    let processor = CommandProcessor::new(config.color, config.debug);
    for command in commands {
        processor
            .execute_command(client, command)
            .await?;
    }
    Ok(())
}

fn print_connect_error(e: &anyhow::Error, config: &AppConfig) {
    if let Some(esl_err) = e.downcast_ref::<EslError>() {
        match esl_err {
            EslError::AuthenticationFailed { reason } => {
                eprintln!("Authentication failed: {}", reason);
            }
            EslError::Io(io_err) => {
                eprintln!(
                    "Failed to connect to FreeSWITCH at {}:{}",
                    config.host, config.port
                );
                match io_err.kind() {
                    std::io::ErrorKind::ConnectionRefused => {
                        eprintln!(
                            "Connection refused - is FreeSWITCH running and listening on port {}?",
                            config.port
                        );
                    }
                    std::io::ErrorKind::TimedOut => {
                        eprintln!("Connection timed out after {} ms", config.timeout);
                    }
                    _ => {
                        eprintln!("IO error: {}", io_err);
                    }
                }
            }
            _ => {
                eprintln!(
                    "Failed to connect to FreeSWITCH at {}:{}",
                    config.host, config.port
                );
                eprintln!("Error: {}", esl_err);
            }
        }
    } else if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
        eprintln!(
            "Failed to connect to FreeSWITCH at {}:{}",
            config.host, config.port
        );
        match io_err.kind() {
            std::io::ErrorKind::ConnectionRefused => {
                eprintln!(
                    "Connection refused - is FreeSWITCH running and listening on port {}?",
                    config.port
                );
            }
            std::io::ErrorKind::TimedOut => {
                eprintln!("Connection timed out after {} ms", config.timeout);
            }
            _ => {
                eprintln!("IO error: {}", io_err);
            }
        }
    } else {
        eprintln!(
            "Failed to connect to FreeSWITCH at {}:{}",
            config.host, config.port
        );
        eprintln!("Error: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_connection_error_with_esl_errors() {
        let err: anyhow::Error = EslError::ConnectionClosed.into();
        assert!(is_connection_error(&err));

        let err: anyhow::Error = EslError::NotConnected.into();
        assert!(is_connection_error(&err));

        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        let err: anyhow::Error = EslError::from(io_err).into();
        assert!(is_connection_error(&err));

        let err: anyhow::Error = EslError::Timeout { timeout_ms: 1000 }.into();
        assert!(!is_connection_error(&err));

        let err: anyhow::Error = EslError::auth_failed("bad password").into();
        assert!(!is_connection_error(&err));
    }

    #[test]
    fn test_is_connection_error_with_io_errors() {
        for error_kind in [
            std::io::ErrorKind::ConnectionRefused,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::ConnectionAborted,
            std::io::ErrorKind::BrokenPipe,
            std::io::ErrorKind::TimedOut,
            std::io::ErrorKind::UnexpectedEof,
        ] {
            let io_err = std::io::Error::new(error_kind, "test");
            let err: anyhow::Error = io_err.into();
            assert!(
                is_connection_error(&err),
                "{:?} should be a connection error",
                error_kind
            );
        }

        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "test");
        let err: anyhow::Error = io_err.into();
        assert!(!is_connection_error(&err));
    }

    #[test]
    fn test_is_connection_error_with_other_errors() {
        let err = anyhow::anyhow!("Some random error");
        assert!(!is_connection_error(&err));
    }
}
