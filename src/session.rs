//! Interactive session management
//!
//! Owns the main select! loop, event consumer task, and reconnection logic.

use crate::channel_info::ChannelProvider;
use crate::commands::CommandProcessor;
use crate::config::AppConfig;
use crate::console_complete::get_console_complete;
use crate::esl_debug::EslDebugLevel;
use crate::log_display::{display_log_event, is_log_event};
use crate::printer::Printer;
use crate::readline::{build_macros, parse_function_key, run_readline_loop, CompletionRequest};
use crate::{
    connect_retry_forever, enable_logging, is_connection_error, is_permission_denied,
    subscribe_heartbeat, subscribe_to_events,
};
use anyhow::Result;
use colored::Colorize;
use crossterm::{
    cursor::MoveTo,
    terminal::{Clear, ClearType},
    ExecutableCommand,
};
use freeswitch_esl_tokio::{
    ConnectionStatus, EslClient, EslEventStream, EslEventType, HeaderLookup,
};
use std::collections::HashMap;
use std::io::{self, Write};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tracing::{debug, error, info, warn};

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
    /// Connection lost. A liveness timeout (heartbeats stopped on a connection
    /// that had them) arrives here too, via `DisconnectReason::HeartbeatExpired`
    /// stringified into the reason — treated like any disconnect, so it honors
    /// `--reconnect`. Liveness is only ever enabled when a HEARTBEAT
    /// subscription succeeded, so a timeout means a genuinely stalled socket.
    Disconnected(Option<String>),
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
    let (printer_tx, printer_rx) = oneshot::channel::<Printer>();
    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<CompletionRequest>();

    setup_subscriptions(&client, config).await;
    println!("FreeSWITCH CLI ready. Type 'help' for commands, '/quit' to exit.\n");

    let macros = build_macros(config);

    #[cfg(unix)]
    let original_termios = save_terminal_state();

    let config_clone = config.clone();
    let readline_handle = tokio::task::spawn_blocking(move || {
        run_readline_loop(cmd_tx, quit_tx, printer_tx, completion_tx, &config_clone)
    });

    let printer = match printer_rx.await {
        Ok(p) => p,
        Err(_) => {
            error!("Failed to receive external printer");
            Printer::none()
        }
    };
    processor.set_printer(printer.clone());

    let channel_provider = ChannelProvider::new(config.max_auto_complete_uuid);

    let mut ctx = CommandLoopCtx {
        processor: &processor,
        macros: &macros,
        channel_provider: &channel_provider,
        config,
        cmd_rx: &mut cmd_rx,
        quit_rx: &mut quit_rx,
        completion_rx: &mut completion_rx,
    };

    // Reconnection loop — each iteration is one connection session
    let session_result = loop {
        let mut event_task =
            spawn_event_consumer(events, printer.clone(), config.color, config.debug);

        let result = run_command_loop(&client, &mut ctx, &mut event_task).await;

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
                let (new_client, new_events) = connect_retry_forever(config).await;
                info!("Reconnected successfully");
                client = new_client;
                events = new_events;
                setup_subscriptions(&client, config).await;
                continue;
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

/// Subscribe to the events this session needs and enable the idle-liveness
/// timer only when a HEARTBEAT subscription is permitted. A permission-
/// restricted user (`esl-allowed-events` without HEARTBEAT) gets
/// `-ERR permission denied`: warn and run without idle-liveness so the timer
/// can't trip on a healthy idle socket. Runs for the initial connection and
/// every reconnect.
async fn setup_subscriptions(client: &EslClient, config: &AppConfig) {
    let subscription = if config.events {
        subscribe_to_events(client).await
    } else {
        subscribe_heartbeat(client).await
    };
    match subscription {
        Ok(()) => client.set_liveness_timeout(LIVENESS_TIMEOUT),
        Err(e) if is_permission_denied(&e) => {
            warn!(
                "event subscription denied ({}); idle-liveness disabled for this user",
                e
            );
        }
        Err(e) => warn!("Failed to subscribe to events: {}", e),
    }
    if !config.quiet {
        if let Err(e) = enable_logging(client, config.log_level).await {
            warn!("Failed to enable logging: {}", e);
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

    let channel = event
        .channel_name()
        .unwrap_or("unknown");
    let uuid = event
        .unique_id()
        .unwrap_or("?");

    let line = if event_type == EslEventType::ChannelHangup {
        let cause_str = match event.hangup_cause() {
            Ok(Some(c)) => c.to_string(),
            Ok(None) => "unknown".to_string(),
            Err(e) => e.to_string(),
        };
        format!("[{}] {} {} ({})", label, uuid, channel, cause_str)
    } else {
        let cid_num = event
            .caller_id_number()
            .unwrap_or("");
        let cid_name = event
            .caller_id_name()
            .unwrap_or("");
        if !cid_num.is_empty() || !cid_name.is_empty() {
            format!(
                "[{}] {} {} <{}> {}",
                label, uuid, channel, cid_num, cid_name
            )
        } else {
            format!("[{}] {} {}", label, uuid, channel)
        }
    };

    Some(match color_mode {
        crate::commands::ColorMode::Never => line,
        _ => line
            .cyan()
            .to_string(),
    })
}

/// Spawn a task that consumes events and displays log/channel messages
fn spawn_event_consumer(
    mut events: EslEventStream,
    printer: Printer,
    color_mode: crate::commands::ColorMode,
    debug_level: EslDebugLevel,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(result) = events
            .recv()
            .await
        {
            match result {
                Ok(event) => {
                    if let Some(raw) = event.raw_body() {
                        info!(
                            "Event body contained invalid UTF-8 ({} bytes), shown with \u{FFFD} replacements",
                            raw.len()
                        );
                        if debug_level >= EslDebugLevel::Debug5 {
                            debug!("Non-UTF-8 body bytes: {}", raw.escape_ascii());
                        }
                    }
                    if let Some(msg) = format_channel_event(&event, color_mode) {
                        printer.print(msg);
                    } else if is_log_event(&event) {
                        display_log_event(&event, color_mode, &printer);
                    }
                }
                Err(e) => {
                    warn!("Event stream error: {}", e);
                }
            }
        }
    })
}

/// Session-lifetime state shared across reconnect iterations.
///
/// Per-connection resources (`client`, `event_task`) are passed separately to
/// `run_command_loop` so they can be swapped on reconnect without rebuilding
/// this struct.
struct CommandLoopCtx<'a> {
    processor: &'a CommandProcessor,
    macros: &'a HashMap<String, String>,
    channel_provider: &'a ChannelProvider,
    config: &'a AppConfig,
    cmd_rx: &'a mut mpsc::UnboundedReceiver<String>,
    quit_rx: &'a mut oneshot::Receiver<()>,
    completion_rx: &'a mut mpsc::UnboundedReceiver<CompletionRequest>,
}

/// Main command processing select! loop for one connection session.
async fn run_command_loop(
    client: &EslClient,
    ctx: &mut CommandLoopCtx<'_>,
    event_task: &mut JoinHandle<()>,
) -> SessionEnd {
    loop {
        tokio::select! {
            result = &mut *event_task => {
                match result {
                    Err(ref e) if e.is_panic() => error!("Event consumer task panicked: {}", e),
                    Err(ref e) => error!("Event consumer task exited unexpectedly: {}", e),
                    Ok(()) => {}
                }
                return match client.status() {
                    ConnectionStatus::Disconnected(r) => {
                        SessionEnd::Disconnected(Some(r.to_string()))
                    }
                    _ => SessionEnd::Disconnected(None),
                };
            }
            Some(command) = ctx.cmd_rx.recv() => {
                if let Some(end) = handle_command_line(ctx.processor, ctx.macros, client, command).await {
                    return end;
                }
            }
            Some(request) = ctx.completion_rx.recv() => {
                let completions = get_console_complete(
                    client, &request.line, request.pos,
                    ctx.config.debug, ctx.channel_provider,
                ).await;
                let _ = request.response_tx.send(completions);
            }
            _ = &mut *ctx.quit_rx => {
                return SessionEnd::Quit;
            }
        }
    }
}

/// Dispatch one line from the readline thread. Returns `Some(end)` if the
/// session should terminate, `None` to continue.
async fn handle_command_line(
    processor: &CommandProcessor,
    macros: &HashMap<String, String>,
    client: &EslClient,
    command: String,
) -> Option<SessionEnd> {
    if command.starts_with('/') {
        return match command.as_str() {
            "/help" => {
                processor.show_help(macros);
                None
            }
            "/clear" => {
                let mut stdout = io::stdout();
                let result: io::Result<()> = (|| {
                    stdout.execute(Clear(ClearType::All))?;
                    stdout.execute(MoveTo(0, 0))?;
                    stdout.flush()
                })();
                if let Err(e) = result {
                    warn!("Failed to clear terminal: {}", e);
                }
                None
            }
            _ => execute_with_disconnect_check(client, processor, &command).await,
        };
    }
    if command == "help" {
        processor.show_help(macros);
        return None;
    }
    let effective = parse_function_key(&command, macros).unwrap_or(command);
    execute_with_disconnect_check(client, processor, &effective).await
}

/// Execute a command and check for connection errors.
/// Returns Some(SessionEnd) if the session should end, None to continue.
async fn execute_with_disconnect_check(
    client: &EslClient,
    processor: &CommandProcessor,
    command: &str,
) -> Option<SessionEnd> {
    if let Err(e) = processor
        .execute_command(client, command)
        .await
    {
        if is_connection_error(&e) {
            return Some(SessionEnd::Disconnected(Some(e.to_string())));
        }
        processor.handle_error(e);
    }
    None
}
