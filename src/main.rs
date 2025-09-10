//! fs_cli-rs: Interactive FreeSWITCH CLI client using ESL
//!
//! A modern Rust-based FreeSWITCH CLI client with readline capabilities,
//! command history, and comprehensive logging.

use anyhow::{Context, Result};
use crossterm::{
    cursor::MoveTo,
    terminal::{Clear, ClearType},
    ExecutableCommand,
};
use freeswitch_esl_rs::{EslEventType, EslHandle, EventFormat};
use gethostname::gethostname;
use rustyline::history::FileHistory;
use rustyline::{Cmd, Editor, ExternalPrinter, KeyCode, KeyEvent, Modifiers};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::{timeout, Duration};
use tracing::{error, info, warn};

mod args;
mod commands;
mod completion;
mod config;
mod esl_debug;
mod log_display;

use args::Args;
use commands::CommandProcessor;
use completion::FsCliCompleter;
use config::AppConfig;
use esl_debug::EslDebugLevel;
use log_display::LogDisplay;

/// Default FreeSWITCH function key bindings
fn get_default_fnkeys() -> HashMap<String, String> {
    let mut macros = HashMap::new();
    macros.insert("f1".to_string(), "help".to_string());
    macros.insert("f2".to_string(), "status".to_string());
    macros.insert("f3".to_string(), "show channels".to_string());
    macros.insert("f4".to_string(), "show calls".to_string());
    macros.insert("f5".to_string(), "sofia status".to_string());
    macros.insert("f6".to_string(), "reloadxml".to_string());
    macros.insert("f7".to_string(), "/log console".to_string());
    macros.insert("f8".to_string(), "/log debug".to_string());
    macros.insert(
        "f9".to_string(),
        "sofia status profile internal".to_string(),
    );
    macros.insert("f10".to_string(), "fsctl pause".to_string());
    macros.insert("f11".to_string(), "fsctl resume".to_string());
    macros.insert("f12".to_string(), "version".to_string());
    macros
}

/// Parse function key shortcuts (F1-F12) with custom macros
fn parse_function_key(input: &str, macros: &HashMap<String, String>) -> Option<String> {
    let key = input.to_lowercase();
    macros.get(&key).cloned()
}

/// Set up function key bindings for readline with custom macros
fn setup_function_key_bindings(
    rl: &mut Editor<FsCliCompleter, FileHistory>,
    macros: &HashMap<String, String>,
) -> Result<()> {
    // Bind F1-F12 to Cmd::Macro for automatic execution
    for i in 1..=12 {
        let key = format!("f{}", i);
        if let Some(command) = macros.get(&key) {
            let f_key = KeyEvent(KeyCode::F(i as u8), Modifiers::NONE);
            // Use Cmd::Macro to replay the command + newline (which triggers AcceptLine)
            rl.bind_sequence(f_key, Cmd::Macro(format!("{}\n", command)));
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Args::parse_and_merge()?;

    // Initialize logging
    setup_logging(config.debug)?;

    // Connect to FreeSWITCH with optional retry
    config
        .debug
        .debug_print(EslDebugLevel::Debug, "About to connect to FreeSWITCH");
    let mut handle = match connect_to_freeswitch_with_retry(&config).await {
        Ok(handle) => {
            config
                .debug
                .debug_print(EslDebugLevel::Debug, "Successfully connected to FreeSWITCH");
            handle
        }
        Err(e) => {
            eprintln!(
                "Failed to connect to FreeSWITCH at {}:{}",
                config.host, config.port
            );
            if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
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
                        eprintln!("Connection error: {}", io_err);
                    }
                }
            } else {
                eprintln!("Error: {}", e);
            }
            std::process::exit(1);
        }
    };

    // Execute commands or start interactive mode
    if !config.execute.is_empty() {
        // For -x mode: execute commands without subscribing to events or logging
        execute_commands(&mut handle, &config.execute, &config).await?;
        // Clean disconnect
        info!("Disconnecting from FreeSWITCH...");
        handle.disconnect().await?;
    } else {
        // Interactive mode: subscribe to events and enable logging if requested
        if config.events {
            config
                .debug
                .debug_print(EslDebugLevel::Debug, "Subscribing to events");
            subscribe_to_events(&mut handle).await?;
        }

        if !config.quiet {
            config.debug.debug_print(
                EslDebugLevel::Debug,
                &format!("Enabling logging at level: {}", config.log_level.as_str()),
            );
            enable_logging(&mut handle, config.log_level).await?;
        }

        run_interactive_mode(handle, &config).await?;
        // Handle is consumed by run_interactive_mode, no need to disconnect
    }

    Ok(())
}

/// Set up logging based on debug level
fn setup_logging(debug_level: EslDebugLevel) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(debug_level.tracing_filter())
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();

    Ok(())
}

/// Connect to FreeSWITCH with timeout
async fn connect_to_freeswitch(config: &AppConfig) -> Result<EslHandle> {
    info!(
        "Connecting to FreeSWITCH at {}:{}",
        config.host, config.port
    );

    let connect_result = if let Some(ref user) = config.user {
        info!("Using user authentication: {}", user);
        timeout(
            Duration::from_millis(config.timeout),
            EslHandle::connect_with_user(&config.host, config.port, user, &config.password),
        )
        .await
    } else {
        info!("Using password authentication");
        timeout(
            Duration::from_millis(config.timeout),
            EslHandle::connect(&config.host, config.port, &config.password),
        )
        .await
    };

    let handle = connect_result
        .context("Connection timed out")?
        .context("Failed to connect to FreeSWITCH")?;

    Ok(handle)
}

/// Connect to FreeSWITCH with optional retry logic
async fn connect_to_freeswitch_with_retry(config: &AppConfig) -> Result<EslHandle> {
    if !config.retry {
        return connect_to_freeswitch(config).await;
    }

    info!(
        "Retry mode enabled - will retry every {} ms",
        config.timeout
    );

    loop {
        match connect_to_freeswitch(config).await {
            Ok(handle) => return Ok(handle),
            Err(e) => {
                warn!("Connection attempt failed: {}", e);
                info!("Retrying in {} ms...", config.timeout);
                tokio::time::sleep(Duration::from_millis(config.timeout)).await;
            }
        }
    }
}

/// Check if error indicates connection loss
fn is_connection_error(error: &anyhow::Error) -> bool {
    if let Some(io_err) = error.downcast_ref::<std::io::Error>() {
        matches!(
            io_err.kind(),
            std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::UnexpectedEof
        )
    } else {
        false
    }
}

/// Attempt to reconnect if connection is lost and reconnect is enabled
async fn handle_reconnection(handle_arc: &Arc<Mutex<EslHandle>>, config: &AppConfig) -> Result<()> {
    if !config.reconnect {
        return Err(anyhow::anyhow!("Connection lost and reconnect disabled"));
    }

    warn!("Connection lost, attempting to reconnect...");

    loop {
        match connect_to_freeswitch(config).await {
            Ok(new_handle) => {
                info!("Reconnected successfully");
                let mut handle = handle_arc.lock().await;
                *handle = new_handle;
                return Ok(());
            }
            Err(e) => {
                warn!("Reconnection attempt failed: {}", e);
                info!("Retrying reconnection in {} ms...", config.timeout);
                tokio::time::sleep(Duration::from_millis(config.timeout)).await;
            }
        }
    }
}

/// Subscribe to events for monitoring
async fn subscribe_to_events(handle: &mut EslHandle) -> Result<()> {
    info!("Subscribing to events...");

    handle
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
async fn enable_logging(
    handle: &mut EslHandle,
    log_level: crate::commands::LogLevel,
) -> Result<()> {
    info!("Enabling logging at level: {}", log_level.as_str());

    use freeswitch_esl_rs::command::EslCommand;

    let cmd = if log_level == crate::commands::LogLevel::NoLog {
        EslCommand::Api {
            command: "nolog".to_string(),
        }
    } else {
        EslCommand::Log {
            level: log_level.as_str().to_string(),
        }
    };

    let response = handle.send_command(cmd).await?;

    if !response.is_success() {
        if let Some(reply) = response.reply_text() {
            warn!("Failed to set log level: {}", reply);
        }
    }

    Ok(())
}

/// Execute multiple commands and exit
async fn execute_commands(
    handle: &mut EslHandle,
    commands: &[String],
    config: &AppConfig,
) -> Result<()> {
    let processor = CommandProcessor::new(config.color, config.debug);

    for command in commands {
        processor.execute_command(handle, command).await?;
    }

    Ok(())
}

/// Completion request from readline thread to main thread
#[derive(Debug)]
pub struct CompletionRequest {
    pub line: String,
    pub pos: usize,
    pub response_tx: oneshot::Sender<Vec<String>>,
}

/// Get console completions from FreeSWITCH using the console_complete API
async fn get_console_complete(
    handle: &mut EslHandle,
    line: &str,
    pos: usize,
    debug_level: EslDebugLevel,
) -> Vec<String> {
    // Build the console_complete command
    let cmd = if pos > 0 && pos < line.len() {
        format!("console_complete c={};{}", pos, line)
    } else {
        format!("console_complete {}", line)
    };

    // ESL Debug level 6: Log console_complete API calls and responses
    debug_level.debug_print(EslDebugLevel::Debug6, &format!("ESL API: {}", cmd));

    // Execute the API command
    match handle.api(&cmd).await {
        Ok(response) => {
            debug_level.debug_print(
                EslDebugLevel::Debug6,
                &format!("ESL Response success: {}", response.is_success()),
            );

            if let Some(body) = response.body() {
                debug_level.debug_print(
                    EslDebugLevel::Debug6,
                    &format!("ESL Response body (escaped): {:?}", body),
                );
                debug_level.debug_print(
                    EslDebugLevel::Debug6,
                    &format!("ESL Response body (raw):\n---START---\n{}\n---END---", body),
                );
                let parsed_completions = parse_console_complete_response(body);
                debug_level.debug_print(
                    EslDebugLevel::Debug6,
                    &format!("Parsed completions: {:?}", parsed_completions),
                );
                parsed_completions
            } else {
                debug_level.debug_print(
                    EslDebugLevel::Debug6,
                    &format!("ESL Response: no body for command: {}", cmd),
                );
                Vec::new()
            }
        }
        Err(e) => {
            // Log error but don't print to console to avoid interfering with readline
            tracing::debug!("Failed to get console completions: {}", e);
            Vec::new()
        }
    }
}

/// Parse the console_complete response from FreeSWITCH
fn parse_console_complete_response(body: &str) -> Vec<String> {
    let mut completions = Vec::new();

    // Parse bracketed completions like [            channels] and [                chat]
    // FreeSWITCH uses format "[%20s]" so we look for [...] patterns
    for line in body.lines() {
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '[' {
                // Found start of bracket, collect until ']'
                let mut bracket_content = String::new();
                for inner_ch in chars.by_ref() {
                    if inner_ch == ']' {
                        break;
                    }
                    bracket_content.push(inner_ch);
                }

                // Clean up the content (remove padding spaces)
                let option_text = bracket_content.trim();
                if !option_text.is_empty() {
                    completions.push(option_text.to_string());
                }
            }
        }
    }

    // If we found bracketed completions, use those
    if !completions.is_empty() {
        return completions;
    }

    // Fallback: Handle write= directive only if no bracketed completions found
    if let Some(write_start) = body.find("write=") {
        let write_section = &body[write_start + 6..]; // Skip "write="
        if let Some(colon_pos) = write_section.find(':') {
            let replacement_text = write_section[colon_pos + 1..].trim_end();
            if !replacement_text.is_empty() {
                completions.push(format!("WRITE_DIRECTIVE:{}", replacement_text));
            }
        }
    }

    completions
}

/// Run the readline loop in a blocking thread
fn run_readline_loop(
    cmd_tx: mpsc::UnboundedSender<String>,
    quit_tx: oneshot::Sender<()>,
    printer_tx: oneshot::Sender<Arc<Mutex<dyn ExternalPrinter + Send>>>,
    completion_tx: mpsc::UnboundedSender<CompletionRequest>,
    config: &AppConfig,
) -> Result<()> {
    // Set up readline editor with completion configuration
    let rl_config = rustyline::Config::builder()
        .completion_type(rustyline::CompletionType::List)
        .completion_show_all_if_ambiguous(true) // Show list on first tab
        .build();
    let mut rl = Editor::<FsCliCompleter, FileHistory>::with_config(rl_config)?;

    // Create completer and provide completion channel
    let completer = FsCliCompleter::new_with_completion_channel(completion_tx, config.debug);
    rl.set_helper(Some(completer));

    // Merge default macros with custom ones
    let mut macros = get_default_fnkeys();
    for (key, value) in &config.macros {
        macros.insert(key.clone(), value.clone());
    }

    // Set up function key bindings with custom macros
    setup_function_key_bindings(&mut rl, &macros)?;

    // Create external printer for background log output
    let printer = rl.create_external_printer()?;
    let printer_arc = Arc::new(Mutex::new(printer));

    // Send printer to main thread
    let _ = printer_tx.send(printer_arc);

    // Load history
    let history_file = config.history_file.clone().unwrap_or_else(|| {
        let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push(".fs_cli_history");
        path
    });

    if history_file.exists() {
        if let Err(e) = rl.load_history(&history_file) {
            warn!("Could not load history: {}", e);
        }
    }

    // Readline loop
    loop {
        let prompt_host = if config.host == "localhost" {
            gethostname().to_string_lossy().to_string()
        } else {
            config.host.clone()
        };
        let prompt = format!("freeswitch@{}> ", prompt_host);

        match rl.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                // Add to history
                let _ = rl.add_history_entry(line);

                // Handle quit commands
                if matches!(line, "/quit" | "/exit" | "/bye") {
                    println!("Goodbye!");
                    let _ = quit_tx.send(());
                    break;
                }

                // Handle history command locally (since we have access to rl here)
                if line == "/history" {
                    println!("Command History:");
                    let history = rl.history();
                    for (i, entry) in history
                        .iter()
                        .enumerate()
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .take(20)
                    {
                        println!("  {}: {}", i + 1, entry);
                    }
                    continue;
                }

                // Send command to main thread
                if cmd_tx.send(line.to_string()).is_err() {
                    break; // Main thread has closed
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                println!("Goodbye!");
                let _ = quit_tx.send(());
                break;
            }
            Err(e) => {
                error!("Error reading input: {}", e);
                break;
            }
        }
    }

    // Save history
    if let Err(e) = rl.save_history(&history_file) {
        warn!("Could not save history: {}", e);
    }

    Ok(())
}

/// Run interactive CLI mode
async fn run_interactive_mode(handle: EslHandle, config: &AppConfig) -> Result<()> {
    let mut processor = CommandProcessor::new(config.color, config.debug);

    // Create channels for communication between rustyline thread and main async thread
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();
    let (quit_tx, mut quit_rx) = oneshot::channel::<()>();
    let (printer_tx, printer_rx) = oneshot::channel::<Arc<Mutex<dyn ExternalPrinter + Send>>>();
    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<CompletionRequest>();

    println!("FreeSWITCH CLI ready. Type 'help' for commands, '/quit' to exit.\n");

    // Prepare macros for function key parsing
    let mut macros = get_default_fnkeys();
    for (key, value) in &config.macros {
        macros.insert(key.clone(), value.clone());
    }

    // Spawn rustyline in a blocking thread
    let config_clone = config.clone();
    let readline_handle = tokio::task::spawn_blocking(move || {
        run_readline_loop(cmd_tx, quit_tx, printer_tx, completion_tx, &config_clone)
    });

    // Wait for external printer to be ready
    let external_printer = match printer_rx.await {
        Ok(printer) => Some(printer),
        Err(_) => {
            error!("Failed to receive external printer");
            None
        }
    };

    // Set the external printer on the command processor
    processor.set_printer(external_printer.clone());

    // Wrap handle in Arc<Mutex> for sharing between tasks
    let handle_arc = Arc::new(Mutex::new(handle));
    let log_handle = if !config.quiet {
        let handle_clone = handle_arc.clone();
        let color_mode = config.color;
        let printer_clone = external_printer.clone();
        let config_clone = config.clone();
        Some(tokio::spawn(async move {
            loop {
                {
                    let mut h = handle_clone.lock().await;
                    if let Err(e) = LogDisplay::check_and_display_logs(
                        &mut h,
                        color_mode,
                        printer_clone.clone(),
                    )
                    .await
                    {
                        if is_connection_error(&e) && config_clone.reconnect {
                            warn!("Connection lost in log monitoring, attempting reconnect...");
                            drop(h); // Release the lock before reconnection
                            if let Err(reconnect_err) =
                                handle_reconnection(&handle_clone, &config_clone).await
                            {
                                warn!("Failed to reconnect in log monitoring: {}", reconnect_err);
                            }
                        } else {
                            warn!("Error in background log monitoring: {}", e);
                        }
                    }
                }
                // Small delay to prevent excessive CPU usage
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }))
    } else {
        None
    };

    // Main command processing loop
    loop {
        tokio::select! {
            // Handle commands from readline thread
            Some(command) = cmd_rx.recv() => {
                let mut handle = handle_arc.lock().await;

                // Handle client-side commands first (start with /)
                if command.starts_with('/') {
                    match command.as_str() {
                        "/help" => {
                            processor.show_help().await;
                            continue;
                        }
                        "/clear" => {
                            // Clear the screen using crossterm
                            let mut stdout = io::stdout();
                            let _ = stdout.execute(Clear(ClearType::All));
                            let _ = stdout.execute(MoveTo(0, 0));
                            let _ = stdout.flush();
                            continue;
                        }
                        _ => {
                            // Let the command processor handle other /commands
                            if let Err(e) = processor.execute_command(&mut handle, &command).await {
                                if is_connection_error(&e) {
                                    drop(handle); // Release the lock before reconnection
                                    if let Err(reconnect_err) = handle_reconnection(&handle_arc, config).await {
                                        processor.handle_error(reconnect_err).await;
                                        continue;
                                    }
                                    // Retry the command after successful reconnection
                                    let mut handle = handle_arc.lock().await;
                                    if let Err(retry_err) = processor.execute_command(&mut handle, &command).await {
                                        processor.handle_error(retry_err).await;
                                    }
                                } else {
                                    processor.handle_error(e).await;
                                }
                            }
                            continue;
                        }
                    }
                }

                // Handle other built-in commands
                match command.as_str() {
                    "help" => {
                        processor.show_help().await;
                        continue;
                    }
                    _ => {
                        // Check for function key shortcuts (F1-F12) typed manually
                        if let Some(fn_command) = parse_function_key(&command, &macros) {
                            if let Err(e) = processor.execute_command(&mut handle, &fn_command).await {
                                if is_connection_error(&e) {
                                    drop(handle); // Release the lock before reconnection
                                    if let Err(reconnect_err) = handle_reconnection(&handle_arc, config).await {
                                        processor.handle_error(reconnect_err).await;
                                        continue;
                                    }
                                    // Retry the command after successful reconnection
                                    let mut handle = handle_arc.lock().await;
                                    if let Err(retry_err) = processor.execute_command(&mut handle, &fn_command).await {
                                        processor.handle_error(retry_err).await;
                                    }
                                } else {
                                    processor.handle_error(e).await;
                                }
                            }
                            continue;
                        }

                        // Execute FreeSWITCH command and show output immediately
                        if let Err(e) = processor.execute_command(&mut handle, &command).await {
                            if is_connection_error(&e) {
                                drop(handle); // Release the lock before reconnection
                                if let Err(reconnect_err) = handle_reconnection(&handle_arc, config).await {
                                    processor.handle_error(reconnect_err).await;
                                    continue;
                                }
                                // Retry the command after successful reconnection
                                let mut handle = handle_arc.lock().await;
                                if let Err(retry_err) = processor.execute_command(&mut handle, &command).await {
                                    processor.handle_error(retry_err).await;
                                }
                            } else {
                                processor.handle_error(e).await;
                            }
                        }
                    }
                }
            }
            // Handle completion requests from readline thread
            Some(request) = completion_rx.recv() => {
                let mut handle = handle_arc.lock().await;
                let completions = get_console_complete(&mut handle, &request.line, request.pos, config.debug).await;
                // Send the result back (ignore if channel closed)
                let _ = request.response_tx.send(completions);
            }
            // Handle quit signal from readline thread
            _ = &mut quit_rx => {
                break;
            }
        }
    }

    // Clean up background tasks
    if let Some(handle) = log_handle {
        handle.abort();
    }

    // Wait for readline thread to finish
    if let Err(e) = readline_handle.await {
        warn!("Error waiting for readline thread: {}", e);
    }

    // Clean disconnect
    info!("Disconnecting from FreeSWITCH...");

    // Try to unwrap the Arc, but handle the case where other references exist
    match Arc::try_unwrap(handle_arc) {
        Ok(mutex) => {
            let mut handle = mutex.into_inner();
            if let Err(e) = handle.disconnect().await {
                warn!("Error during clean disconnect: {}", e);
            }
        }
        Err(arc) => {
            // If we can't unwrap, just disconnect through the Arc
            let mut handle = arc.lock().await;
            if let Err(e) = handle.disconnect().await {
                warn!("Error during disconnect: {}", e);
            }
        }
    }

    Ok(())
}
