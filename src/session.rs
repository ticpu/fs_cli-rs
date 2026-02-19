//! Interactive session management
//!
//! Owns the main select! loop, event consumer task, and reconnection logic.

use crate::channel_info::ChannelProvider;
use crate::commands::CommandProcessor;
use crate::config::AppConfig;
use crate::console_complete::get_console_complete;
use crate::log_display::LogDisplay;
use crate::readline::{build_macros, parse_function_key, run_readline_loop, CompletionRequest};
use crate::{connect_to_freeswitch, enable_logging, is_connection_error, subscribe_to_events};
use anyhow::Result;
use crossterm::{
    cursor::MoveTo,
    terminal::{Clear, ClearType},
    ExecutableCommand,
};
use freeswitch_esl_rs::{EslClient, EslEventStream};
use rustyline::ExternalPrinter;
use std::io::{self, Write};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

/// Why the command loop exited
enum SessionEnd {
    Quit,
    Disconnected,
}

/// Run interactive CLI mode with reconnection support
pub async fn run_interactive_mode(
    mut client: EslClient,
    mut events: EslEventStream,
    config: &AppConfig,
) -> Result<()> {
    let mut processor = CommandProcessor::new(config.color, config.debug);

    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();
    let (quit_tx, mut quit_rx) = oneshot::channel::<()>();
    let (printer_tx, printer_rx) = oneshot::channel::<Arc<Mutex<dyn ExternalPrinter + Send>>>();
    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<CompletionRequest>();

    println!("FreeSWITCH CLI ready. Type 'help' for commands, '/quit' to exit.\n");

    let macros = build_macros(config);

    let config_clone = config.clone();
    let readline_handle = tokio::task::spawn_blocking(move || {
        run_readline_loop(cmd_tx, quit_tx, printer_tx, completion_tx, &config_clone)
    });

    let external_printer = match printer_rx.await {
        Ok(printer) => Some(printer),
        Err(_) => {
            error!("Failed to receive external printer");
            None
        }
    };
    processor.set_printer(external_printer.clone());

    let channel_provider = ChannelProvider::new(config.max_auto_complete_uuid);

    // Reconnection loop — each iteration is one connection session
    let session_result = loop {
        let mut event_task = spawn_event_consumer(events, external_printer.clone(), config.color);

        let result = run_command_loop(
            &client,
            &processor,
            &macros,
            &channel_provider,
            config,
            &mut cmd_rx,
            &mut quit_rx,
            &mut completion_rx,
            &mut event_task,
        )
        .await;

        event_task.abort();

        match result {
            SessionEnd::Quit => {
                client
                    .disconnect()
                    .await
                    .ok();
                break Ok(());
            }
            SessionEnd::Disconnected => {
                if !config.reconnect {
                    break Err(anyhow::anyhow!("Connection to FreeSWITCH lost"));
                }
                warn!("Connection lost, reconnecting...");
                match reconnect_loop(config).await {
                    Ok((new_client, new_events)) => {
                        client = new_client;
                        events = new_events;
                        setup_subscriptions(&client, config).await;
                        continue;
                    }
                    Err(e) => break Err(e),
                }
            }
        }
    };

    readline_handle.abort();
    if let Err(e) = readline_handle.await {
        if !e.is_cancelled() {
            warn!("Error waiting for readline thread: {}", e);
        }
    }

    session_result
}

/// Re-subscribe to events and re-enable logging after reconnection
async fn setup_subscriptions(client: &EslClient, config: &AppConfig) {
    if config.events {
        if let Err(e) = subscribe_to_events(client).await {
            warn!("Failed to re-subscribe to events: {}", e);
        }
    }
    if !config.quiet {
        if let Err(e) = enable_logging(client, config.log_level).await {
            warn!("Failed to re-enable logging: {}", e);
        }
    }
}

/// Retry connecting until success
async fn reconnect_loop(config: &AppConfig) -> Result<(EslClient, EslEventStream)> {
    loop {
        match connect_to_freeswitch(config).await {
            Ok(pair) => {
                info!("Reconnected successfully");
                return Ok(pair);
            }
            Err(e) => {
                warn!("Reconnection attempt failed: {}", e);
                info!("Retrying in {} ms...", config.timeout);
                tokio::time::sleep(tokio::time::Duration::from_millis(config.timeout)).await;
            }
        }
    }
}

/// Spawn a task that consumes events and displays log messages
fn spawn_event_consumer(
    mut events: EslEventStream,
    printer: Option<Arc<Mutex<dyn ExternalPrinter + Send>>>,
    color_mode: crate::commands::ColorMode,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(result) = events
            .recv()
            .await
        {
            match result {
                Ok(event) => {
                    if LogDisplay::is_log_event(&event) {
                        LogDisplay::display_log_event(&event, color_mode, &printer).await;
                    }
                }
                Err(e) => {
                    warn!("Event stream error: {}", e);
                }
            }
        }
    })
}

/// Main command processing select! loop for one connection session
#[allow(clippy::too_many_arguments)]
async fn run_command_loop(
    client: &EslClient,
    processor: &CommandProcessor,
    macros: &std::collections::HashMap<String, String>,
    channel_provider: &ChannelProvider,
    config: &AppConfig,
    cmd_rx: &mut mpsc::UnboundedReceiver<String>,
    quit_rx: &mut oneshot::Receiver<()>,
    completion_rx: &mut mpsc::UnboundedReceiver<CompletionRequest>,
    event_task: &mut JoinHandle<()>,
) -> SessionEnd {
    loop {
        tokio::select! {
            // Event consumer finished = connection lost
            _ = &mut *event_task => {
                return SessionEnd::Disconnected;
            }
            Some(command) = cmd_rx.recv() => {
                // Client-side commands
                if command.starts_with('/') {
                    match command.as_str() {
                        "/help" => {
                            processor.show_help().await;
                            continue;
                        }
                        "/clear" => {
                            let mut stdout = io::stdout();
                            let _ = stdout.execute(Clear(ClearType::All));
                            let _ = stdout.execute(MoveTo(0, 0));
                            let _ = stdout.flush();
                            continue;
                        }
                        _ => {
                            if let Some(end) = execute_with_disconnect_check(
                                client, processor, &command, config,
                            ).await {
                                return end;
                            }
                            continue;
                        }
                    }
                }

                if command == "help" {
                    processor.show_help().await;
                    continue;
                }

                // Resolve function key shortcuts typed manually
                let effective = parse_function_key(&command, macros)
                    .unwrap_or(command);

                if let Some(end) = execute_with_disconnect_check(
                    client, processor, &effective, config,
                ).await {
                    return end;
                }
            }
            Some(request) = completion_rx.recv() => {
                let completions = get_console_complete(
                    client, &request.line, request.pos,
                    config.debug, channel_provider,
                ).await;
                let _ = request.response_tx.send(completions);
            }
            _ = &mut *quit_rx => {
                return SessionEnd::Quit;
            }
        }
    }
}

/// Execute a command and check for connection errors.
/// Returns Some(SessionEnd) if the session should end, None to continue.
async fn execute_with_disconnect_check(
    client: &EslClient,
    processor: &CommandProcessor,
    command: &str,
    config: &AppConfig,
) -> Option<SessionEnd> {
    if let Err(e) = processor
        .execute_command(client, command)
        .await
    {
        if is_connection_error(&e) {
            if config.reconnect {
                return Some(SessionEnd::Disconnected);
            }
            error!("Connection to FreeSWITCH lost");
            return Some(SessionEnd::Disconnected);
        }
        processor
            .handle_error(e)
            .await;
    }
    None
}
