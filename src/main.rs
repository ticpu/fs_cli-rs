//! fs_cli-rs: Interactive FreeSWITCH CLI client using ESL

use anyhow::{Context, Result};
use freeswitch_esl_tokio::EslClient;
use std::io::IsTerminal;
use tracing::{debug, info};

mod args;
mod batch;
mod channel_info;
mod client_command;
mod commands;
mod completion;
mod config;
mod connection;
mod console_complete;
mod esl_debug;
mod log_display;
mod log_level;
mod printer;
mod readline;
mod session;

use args::Args;
use config::AppConfig;
use connection::{connect_to_freeswitch_with_retry, print_connect_error};
use esl_debug::EslDebugLevel;
use log_display::LogDestination;

#[tokio::main(flavor = "current_thread")]
// qual:allow(iosp) reason: "entry point wiring the program together; splitting it would invent indirection"
async fn main() -> Result<()> {
    let config = Args::parse_and_merge()?;

    setup_logging(config.debug);

    if !usable_mode(&config) {
        eprintln!(
            "fs_cli: interactive mode needs a terminal on both stdin and stdout.\n\
             Give commands with -x/-X, or a log destination with --log-file PATH."
        );
        std::process::exit(1);
    }

    // Opened before connecting so an unwritable path fails without a session.
    let log_destination = match &config.log_file {
        Some(spec) => Some(LogDestination::open(spec, config.color)?),
        None => None,
    };

    debug!("About to connect to FreeSWITCH");
    let (client, events) = match connect_to_freeswitch_with_retry(&config).await {
        Ok(pair) => {
            debug!("Successfully connected to FreeSWITCH");
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
        batch::run_batch(&client, events, &config, log_destination).await?;
        disconnect(&client).await?;
    } else if terminal_available() {
        if let Err(e) =
            session::run_interactive_mode(client, events, &config, log_destination).await
        {
            eprintln!("{:#}", e);
            std::process::exit(1);
        }
    } else {
        let destination = log_destination.context("interactive mode needs a terminal")?;
        batch::run_streaming(&client, events, &config, destination).await?;
        disconnect(&client).await?;
    }

    Ok(())
}

async fn disconnect(client: &EslClient) -> Result<()> {
    info!("Disconnecting from FreeSWITCH...");
    client
        .disconnect()
        .await?;
    Ok(())
}

/// rustyline needs a terminal on both streams before it will build its external
/// printer, so that pair is what decides whether interactive mode is possible.
fn terminal_available() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Interactive mode is the only mode that needs a terminal.
fn usable_mode(config: &AppConfig) -> bool {
    !config
        .execute
        .is_empty()
        || config
            .log_file
            .is_some()
        || terminal_available()
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
