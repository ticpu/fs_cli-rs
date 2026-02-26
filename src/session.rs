//! Interactive session management
//!
//! Owns the main select! loop, event consumer task, and reconnection logic.

use crate::channel_info::ChannelProvider;
use crate::commands::CommandProcessor;
use crate::config::AppConfig;
use crate::console_complete::get_console_complete;
use crate::log_display::LogDisplay;
use crate::readline::{build_macros, parse_function_key, run_readline_loop, CompletionRequest};
use crate::{connect_to_freeswitch, enable_logging, is_connection_error, subscribe_heartbeat, subscribe_to_events};
use anyhow::Result;
use crossterm::{
    cursor::MoveTo,
    terminal::{Clear, ClearType},
    ExecutableCommand,
};
use freeswitch_esl_tokio::{ConnectionStatus, DisconnectReason, EslClient, EslEventStream, EslEventType, HeaderLookup};
use rustyline::ExternalPrinter;
use std::io::{self, Write};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tracing::{error, info, warn};

const LIVENESS_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(unix)]
fn save_terminal_state() -> Option<libc::termios> {
    use std::mem::MaybeUninit;
    unsafe {
        let mut termios = MaybeUninit::uninit();
        if libc::tcgetattr(libc::STDIN_FILENO, termios.as_mut_ptr()) == 0 {
            Some(termios.assume_init())
        } else {
            None
        }
    }
}

#[cfg(unix)]
fn restore_terminal_state(termios: &libc::termios) {
    unsafe {
        libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, termios);
    }
}

/// Why the command loop exited
enum SessionEnd {
    Quit,
    Disconnected(Option<String>),
    /// Liveness timeout: no traffic from server. Always fatal — reconnecting
    /// to a connection that delivered no heartbeats is unlikely to help, and
    /// it signals a missing heartbeat subscription.
    HeartbeatTimeout,
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
    client.set_liveness_timeout(LIVENESS_TIMEOUT);

    let macros = build_macros(config);

    #[cfg(unix)]
    let original_termios = save_terminal_state();

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

        let dropped = client.dropped_event_count();
        if dropped > 0 {
            warn!("{} events dropped due to full queue", dropped);
        }

        match result {
            SessionEnd::Quit => {
                client
                    .disconnect()
                    .await
                    .ok();
                break Ok(());
            }
            SessionEnd::HeartbeatTimeout => {
                error!("Liveness timeout: no traffic received from server");
                break Err(anyhow::anyhow!("Liveness timeout: connection lost"));
            }
            SessionEnd::Disconnected(reason) => {
                if !config.reconnect {
                    let msg = match &reason {
                        Some(r) => format!("Connection to FreeSWITCH lost: {}", r),
                        None => "Connection to FreeSWITCH lost".to_string(),
                    };
                    break Err(anyhow::anyhow!(msg));
                }
                match &reason {
                    Some(r) => warn!("Connection lost ({}), reconnecting...", r),
                    None => warn!("Connection lost, reconnecting..."),
                }
                match reconnect_loop(config).await {
                    Ok((new_client, new_events)) => {
                        client = new_client;
                        events = new_events;
                        setup_subscriptions(&client, config).await;
                        client.set_liveness_timeout(LIVENESS_TIMEOUT);
                        continue;
                    }
                    Err(e) => break Err(e),
                }
            }
        }
    };

    readline_handle.abort();

    if session_result.is_err() {
        // The readline thread is blocked inside rl.readline() and cannot be
        // interrupted. Restore the terminal ourselves (rustyline won't get the
        // chance) and return immediately; main.rs will call process::exit which
        // kills the detached blocking thread.
        #[cfg(unix)]
        if let Some(ref termios) = original_termios {
            restore_terminal_state(termios);
        }
        return session_result;
    }

    // Clean exit: readline already broke its loop (user typed /quit or EOF),
    // so the handle resolves quickly.
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
    } else if let Err(e) = subscribe_heartbeat(client).await {
        warn!("Failed to re-subscribe to heartbeat: {}", e);
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

fn format_channel_event(
    event: &freeswitch_esl_tokio::EslEvent,
    color_mode: crate::commands::ColorMode,
) -> Option<String> {
    let event_type = event.event_type()?;

    let label = match event_type {
        EslEventType::ChannelCreate => "CREATE",
        EslEventType::ChannelAnswer => "ANSWER",
        EslEventType::ChannelHangup => "HANGUP",
        EslEventType::Heartbeat => return None,
        _ => return None,
    };

    let channel = event.channel_name().unwrap_or("unknown");
    let uuid = event.unique_id().unwrap_or("?");

    let line = if event_type == EslEventType::ChannelHangup {
        let cause = event.hangup_cause();
        format!("[{}] {} {} ({})", label, uuid, channel, cause.map_or_else(|e| e.to_string(), |h| h.unwrap().to_string()))
    } else {
        let cid_num = event.caller_id_number().unwrap_or("");
        let cid_name = event.caller_id_name().unwrap_or("");
        if !cid_num.is_empty() || !cid_name.is_empty() {
            format!("[{}] {} {} <{}> {}", label, uuid, channel, cid_num, cid_name)
        } else {
            format!("[{}] {} {}", label, uuid, channel)
        }
    };

    Some(match color_mode {
        crate::commands::ColorMode::Never => line,
        _ => format!("\x1b[36m{}\x1b[0m", line),
    })
}

fn print_to_printer(
    printer: &Option<Arc<Mutex<dyn ExternalPrinter + Send>>>,
    message: String,
) {
    if let Some(printer_arc) = printer {
        if let Ok(mut p) = printer_arc.try_lock() {
            let _ = p.print(message);
            return;
        }
    }
    println!("{}", message);
}

/// Spawn a task that consumes events and displays log/channel messages
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
                    if let Some(msg) = format_channel_event(&event, color_mode) {
                        print_to_printer(&printer, msg);
                    } else if LogDisplay::is_log_event(&event) {
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
            _ = &mut *event_task => {
                return match client.status() {
                    ConnectionStatus::Disconnected(DisconnectReason::HeartbeatExpired) => {
                        SessionEnd::HeartbeatTimeout
                    }
                    ConnectionStatus::Disconnected(r) => {
                        SessionEnd::Disconnected(Some(r.to_string()))
                    }
                    _ => SessionEnd::Disconnected(None),
                };
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
                return Some(SessionEnd::Disconnected(None));
            }
            error!("Connection to FreeSWITCH lost");
            return Some(SessionEnd::Disconnected(None));
        }
        processor
            .handle_error(e)
            .await;
    }
    None
}
