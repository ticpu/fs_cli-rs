//! fs_cli-rs: Interactive FreeSWITCH CLI client using ESL

use anyhow::Result;
use freeswitch_esl_tokio::EslClient;
use tracing::info;

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
use config::AppConfig;
use connection::{connect_to_freeswitch_with_retry, print_connect_error};
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

async fn execute_commands(
    client: &EslClient,
    commands: &[String],
    config: &AppConfig,
) -> Result<()> {
    let output = printer::Output::new(config.color);
    let processor = CommandProcessor::new(&output, config.debug);
    for command in commands {
        processor
            .execute_command(client, command)
            .await?;
    }
    Ok(())
}
