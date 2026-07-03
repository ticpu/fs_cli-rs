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
mod printer;
mod readline;
mod session;

use args::Args;
use commands::CommandProcessor;
use config::AppConfig;
use esl_debug::EslDebugLevel;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let config = Args::parse_and_merge()?;

    setup_logging(config.debug);

    config
        .debug
        .debug_print(EslDebugLevel::Debug, || {
            "About to connect to FreeSWITCH".to_string()
        });
    let (client, events) = match connect_to_freeswitch_with_retry(&config).await {
        Ok(pair) => {
            config
                .debug
                .debug_print(EslDebugLevel::Debug, || {
                    "Successfully connected to FreeSWITCH".to_string()
                });
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
    } else if let Err(e) = session::run_interactive_mode(client, events, &config).await {
        // Event subscriptions, idle-liveness gating, and logging are set up
        // per-connection inside run_interactive_mode (initial and reconnect).
        eprintln!("{}", e);
        std::process::exit(1);
    }

    Ok(())
}

fn setup_logging(debug_level: EslDebugLevel) {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(debug_level.tracing_filter())
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();
}

/// Connect to FreeSWITCH with timeout
pub async fn connect_to_freeswitch(config: &AppConfig) -> Result<(EslClient, EslEventStream)> {
    info!(
        "Connecting to FreeSWITCH at {}",
        format_host_port(&config.host, config.port)
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

/// Retry connecting forever. Never returns — loops until a connection succeeds.
pub async fn connect_retry_forever(config: &AppConfig) -> (EslClient, EslEventStream) {
    loop {
        match connect_to_freeswitch(config).await {
            Ok(pair) => return pair,
            Err(e) => {
                warn!("Connection attempt failed: {}", e);
                info!("Retrying in {} ms...", config.timeout);
                tokio::time::sleep(Duration::from_millis(config.timeout)).await;
            }
        }
    }
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
    Ok(connect_retry_forever(config).await)
}

/// Check if error indicates connection loss
pub fn is_connection_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<EslError>()
        .is_some_and(|e| e.is_connection_error())
}

/// Check if error is an ESL permission denial (e.g. an event the user is not
/// allowed to subscribe to). Used to gate the idle-liveness timer: a restricted
/// user who can't subscribe to HEARTBEAT has no idle traffic source.
pub fn is_permission_denied(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<EslError>()
        .is_some_and(|e| e.is_permission_denied())
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
    info!("Event monitoring enabled");
    Ok(())
}

/// Subscribe to heartbeat events only, to keep the liveness timer alive
pub async fn subscribe_heartbeat(client: &EslClient) -> Result<()> {
    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::Heartbeat])
        .await?;
    Ok(())
}

/// Enable logging at the specified level
pub async fn enable_logging(
    client: &EslClient,
    log_level: crate::commands::LogLevel,
) -> Result<()> {
    info!("Enabling logging at level: {}", log_level.as_str());
    if let Some(reply) = crate::commands::set_log_level(client, log_level).await? {
        warn!("Failed to set log level: {}", reply);
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

fn format_host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{}]:{}", host, port)
    } else {
        format!("{}:{}", host, port)
    }
}

fn print_io_hint(io_err: &std::io::Error, config: &AppConfig) {
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

fn print_connect_error(e: &anyhow::Error, config: &AppConfig) {
    if let Some(EslError::AuthenticationFailed { reason }) = e.downcast_ref::<EslError>() {
        eprintln!("Authentication failed: {}", reason);
        return;
    }

    eprintln!(
        "Failed to connect to FreeSWITCH at {}",
        format_host_port(&config.host, config.port)
    );

    if let Some(esl_err) = e.downcast_ref::<EslError>() {
        match esl_err {
            EslError::Io(io_err) => print_io_hint(io_err, config),
            _ => eprintln!("Error: {}", esl_err),
        }
    } else if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
        print_io_hint(io_err, config);
    } else {
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
        // IO errors wrapped in EslError are connection errors
        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        let err: anyhow::Error = EslError::from(io_err).into();
        assert!(is_connection_error(&err));

        // Bare io::Error not wrapped in EslError is not recognized
        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        let err: anyhow::Error = io_err.into();
        assert!(!is_connection_error(&err));
    }

    #[test]
    fn test_is_connection_error_with_other_errors() {
        let err = anyhow::anyhow!("Some random error");
        assert!(!is_connection_error(&err));
    }
}
