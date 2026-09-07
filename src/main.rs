//! fs_cli-rs: Interactive FreeSWITCH CLI client using ESL

use anyhow::{Context, Result};
use freeswitch_esl_tokio::EslClient;
use std::io::IsTerminal;
use tracing::{debug, info};

mod args;
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
use commands::CommandProcessor;
use config::{AppConfig, BatchCommand};
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
        execute_commands(&client, &config.execute, &config).await?;
        info!("Disconnecting from FreeSWITCH...");
        client
            .disconnect()
            .await?;
    } else if let Err(e) =
        session::run_interactive_mode(client, events, &config, log_destination).await
    {
        eprintln!("{:#}", e);
        std::process::exit(1);
    }

    Ok(())
}

/// Interactive mode is the only mode that needs a terminal, and rustyline needs
/// one on both streams before it will build its external printer.
fn usable_mode(config: &AppConfig) -> bool {
    !config
        .execute
        .is_empty()
        || config
            .log_file
            .is_some()
        || (std::io::stdin().is_terminal() && std::io::stdout().is_terminal())
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

async fn execute_commands(
    client: &EslClient,
    commands: &[BatchCommand],
    config: &AppConfig,
) -> Result<()> {
    let output = printer::Output::new(config.color);
    let processor = CommandProcessor::new(&output);
    for command in commands {
        match command {
            BatchCommand::Api(cmd) => {
                processor
                    .execute_command(client, cmd)
                    .await?
            }
            BatchCommand::BgApi(cmd) => {
                start_background_job(client, cmd, &processor, &output).await?
            }
        }
    }
    Ok(())
}

/// Submits the job and reports its Job-UUID. Nothing here waits for the
/// BACKGROUND_JOB event that carries the result.
async fn start_background_job(
    client: &EslClient,
    command: &str,
    processor: &CommandProcessor,
    output: &printer::Output,
) -> Result<()> {
    let response = client
        .bgapi(command)
        .await
        .with_context(|| format!("bgapi {}", command))?;
    match response.into_result() {
        Ok(accepted) => match accepted.job_uuid() {
            Some(uuid) => output.print(format!("Job-UUID: {}", uuid)),
            None => output.print_labeled(
                "API Error",
                &format!("bgapi {} was accepted without a Job-UUID", command),
            ),
        },
        Err(e) => {
            processor.handle_error(anyhow::Error::new(e).context(format!("bgapi {}", command)))
        }
    }
    Ok(())
}
