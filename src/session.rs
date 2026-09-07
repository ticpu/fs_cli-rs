//! Interactive session management
//!
//! Owns the main select! loop, event consumer task, and reconnection logic.

use crate::channel_info::ChannelProvider;
use crate::client_command::{ClientCommand, ParseError};
use crate::commands::CommandProcessor;
use crate::completion::CompletionRequest;
use crate::config::AppConfig;
use crate::connection::{
    connect_retry_forever, enable_logging, is_connection_error, is_permission_denied,
    subscribe_heartbeat, subscribe_to_events,
};
use crate::console_complete::get_console_complete;
use crate::log_display::{display_log_event, format_channel_event, is_log_event};
use crate::printer::{Output, Printer};
use crate::readline::{build_macros, parse_function_key, run_readline_loop, ReadlineChannels};
use anyhow::Result;
use crossterm::{
    cursor::MoveTo,
    terminal::{Clear, ClearType},
    ExecutableCommand,
};
use freeswitch_esl_tokio::{ConnectionStatus, DisconnectReason, EslClient, EslEventStream};
use std::collections::HashMap;
use std::io::{self, Write};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tracing::{debug, error, info, trace, warn};

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
    Disconnected(DisconnectCause),
}

/// What told us the connection was gone. A liveness timeout arrives as
/// `HeartbeatExpired`, and is honoured like any other disconnect.
enum DisconnectCause {
    Status(DisconnectReason),
    Command(anyhow::Error),
    Unknown,
}

impl std::fmt::Display for DisconnectCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DisconnectCause::Status(reason) => write!(f, "{}", reason),
            DisconnectCause::Command(e) => write!(f, "{:#}", e),
            DisconnectCause::Unknown => write!(f, "reason unknown"),
        }
    }
}

/// Run interactive CLI mode with reconnection support
pub async fn run_interactive_mode(
    mut client: EslClient,
    mut events: EslEventStream,
    config: &AppConfig,
) -> Result<()> {
    let mut output = Output::new(config.color);

    setup_subscriptions(&client, config).await;
    println!("FreeSWITCH CLI ready. Type 'help' for commands, '/quit' to exit.\n");

    let macros = build_macros(config);
    let (readline_chans, mut chans) = ReadlineChannels::new(macros.clone());

    #[cfg(unix)]
    let original_termios = save_terminal_state();

    let config_clone = config.clone();
    let readline_handle =
        tokio::task::spawn_blocking(move || run_readline_loop(readline_chans, &config_clone));

    let printer = match chans
        .printer
        .await
    {
        Ok(p) => p,
        Err(_) => {
            error!("Failed to receive external printer");
            Printer::none()
        }
    };
    output.set_printer(printer);
    let processor = CommandProcessor::new(&output);

    let channel_provider = ChannelProvider::new(config.max_auto_complete_uuid);

    let mut ctx = CommandLoopCtx {
        parts: SessionParts {
            processor: &processor,
            output: &output,
            macros: &macros,
            channel_provider: &channel_provider,
        },
        cmd_rx: &mut chans.commands,
        quit_rx: &mut chans.quit,
        completion_rx: &mut chans.completions,
    };

    // Reconnection loop — each iteration is one connection session
    let session_result = loop {
        let mut event_task = spawn_event_consumer(events, &output);

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
            SessionEnd::Disconnected(cause) => {
                if !config.reconnect {
                    break Err(anyhow::anyhow!("Connection to FreeSWITCH lost: {}", cause));
                }
                warn!("Connection lost ({}), reconnecting...", cause);
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

/// Spawn a task that consumes events and displays log/channel messages
fn spawn_event_consumer(mut events: EslEventStream, output: &Output) -> JoinHandle<()> {
    let output = output.clone();
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
                        trace!("Non-UTF-8 body bytes: {}", raw.escape_ascii());
                    }
                    if let Some(msg) = format_channel_event(&event, &output) {
                        output.print(msg);
                    } else if is_log_event(&event) {
                        display_log_event(&event, &output);
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
struct SessionParts<'a> {
    processor: &'a CommandProcessor,
    output: &'a Output,
    macros: &'a HashMap<String, String>,
    channel_provider: &'a ChannelProvider,
}

struct CommandLoopCtx<'a> {
    parts: SessionParts<'a>,
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
                        SessionEnd::Disconnected(DisconnectCause::Status(r))
                    }
                    _ => SessionEnd::Disconnected(DisconnectCause::Unknown),
                };
            }
            Some(command) = ctx.cmd_rx.recv() => {
                if let Some(end) = handle_command_line(&ctx.parts, client, command).await {
                    return end;
                }
            }
            Some(request) = ctx.completion_rx.recv() => {
                let completions =
                    get_console_complete(client, &request, ctx.parts.channel_provider).await;
                if let Err(e) = request.response_tx.send(completions) {
                    debug!("completion reply dropped for {:?}: {}", request.line, e);
                }
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
    parts: &SessionParts<'_>,
    client: &EslClient,
    command: String,
) -> Option<SessionEnd> {
    match command.parse::<ClientCommand>() {
        Ok(ClientCommand::Help) => {
            parts
                .processor
                .show_help(parts.macros);
            None
        }
        Ok(ClientCommand::Clear) => {
            clear_terminal();
            None
        }
        // Both run on the readline thread, which owns the history and the quit
        // signal; they only reach here if that parse and this one disagree.
        Ok(ClientCommand::History) | Ok(ClientCommand::Quit) => None,
        Ok(ClientCommand::Log(level)) => match parts
            .processor
            .handle_log_command(client, level)
            .await
        {
            Ok(Some(message)) => {
                parts
                    .output
                    .print(message);
                None
            }
            Ok(None) => None,
            Err(e) => report_or_disconnect(parts.processor, e),
        },
        Err(ParseError::InvalidLogLevel(level)) => {
            parts
                .output
                .print(format!("Invalid log level: {}", level));
            None
        }
        Err(ParseError::NotClientCommand) => {
            let effective = parse_function_key(&command, parts.macros).unwrap_or(command);
            if let Err(e) = parts
                .processor
                .execute_command(client, &effective)
                .await
            {
                return report_or_disconnect(parts.processor, e);
            }
            None
        }
    }
}

fn clear_terminal() {
    let mut stdout = io::stdout();
    let result: io::Result<()> = (|| {
        stdout.execute(Clear(ClearType::All))?;
        stdout.execute(MoveTo(0, 0))?;
        stdout.flush()
    })();
    if let Err(e) = result {
        warn!("Failed to clear terminal: {}", e);
    }
}

/// End the session on a connection error, print anything else.
fn report_or_disconnect(processor: &CommandProcessor, e: anyhow::Error) -> Option<SessionEnd> {
    if is_connection_error(&e) {
        return Some(SessionEnd::Disconnected(DisconnectCause::Command(e)));
    }
    processor.handle_error(e);
    None
}
