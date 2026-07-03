//! Readline thread and function key management

use crate::completion::FsCliCompleter;
use crate::config::AppConfig;
use crate::console_complete::Completion;
use crate::printer::Printer;
use anyhow::Result;
use gethostname::gethostname;
use rustyline::history::{FileHistory, History};
use rustyline::{Cmd, Editor, EventHandler, KeyCode, KeyEvent, Modifiers, Movement};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, warn};

/// Completion request from readline thread to main thread
#[derive(Debug)]
pub struct CompletionRequest {
    pub line: String,
    pub pos: usize,
    pub response_tx: std::sync::mpsc::SyncSender<Vec<Completion>>,
}

/// Default F1-F12 macro bindings in key-sorted order.
pub const DEFAULT_FNKEYS: [(&str, &str); 12] = [
    ("f1", "help"),
    ("f2", "status"),
    ("f3", "show channels"),
    ("f4", "show calls"),
    ("f5", "sofia status"),
    ("f6", "reloadxml"),
    ("f7", "/log console"),
    ("f8", "/log debug"),
    ("f9", "sofia status profile internal"),
    ("f10", "fsctl pause"),
    ("f11", "fsctl resume"),
    ("f12", "version"),
];

/// Default FreeSWITCH function key bindings
pub fn get_default_fnkeys() -> HashMap<String, String> {
    DEFAULT_FNKEYS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Parse function key shortcuts (F1-F12) with custom macros
pub fn parse_function_key(input: &str, macros: &HashMap<String, String>) -> Option<String> {
    let key = input.to_lowercase();
    macros
        .get(&key)
        .cloned()
}

/// Build merged macros from defaults + config overrides
pub fn build_macros(config: &AppConfig) -> HashMap<String, String> {
    let mut macros = get_default_fnkeys();
    for (key, value) in &config.macros {
        macros.insert(key.clone(), value.clone());
    }
    macros
}

fn setup_function_key_bindings(
    rl: &mut Editor<FsCliCompleter, FileHistory>,
    macros: &HashMap<String, String>,
) -> Result<()> {
    for i in 1..=12 {
        let key = format!("f{}", i);
        if let Some(command) = macros.get(&key) {
            let f_key = KeyEvent(KeyCode::F(i as u8), Modifiers::NONE);
            rl.bind_sequence(
                f_key,
                EventHandler::Macro(vec![
                    Cmd::Stash,
                    Cmd::Kill(Movement::WholeLine),
                    Cmd::Insert(1, command.clone()),
                    Cmd::AcceptLine,
                ]),
            );
        }
    }
    Ok(())
}

/// Run the readline loop in a blocking thread
pub fn run_readline_loop(
    cmd_tx: mpsc::UnboundedSender<String>,
    quit_tx: oneshot::Sender<()>,
    printer_tx: oneshot::Sender<Printer>,
    completion_tx: mpsc::UnboundedSender<CompletionRequest>,
    config: &AppConfig,
) -> Result<()> {
    let rl_config = rustyline::Config::builder()
        .completion_type(rustyline::CompletionType::List)
        .completion_show_all_if_ambiguous(true)
        .build();
    let mut rl = Editor::<FsCliCompleter, FileHistory>::with_config(rl_config)?;

    let completer = FsCliCompleter::new(completion_tx, config.debug);
    rl.set_helper(Some(completer));

    let macros = build_macros(config);
    setup_function_key_bindings(&mut rl, &macros)?;

    let printer = rl.create_external_printer()?;
    if printer_tx
        .send(Printer::with_external(printer))
        .is_err()
    {
        warn!("Session ended before printer was delivered");
    }

    let history_file = config
        .history_file
        .clone()
        .unwrap_or_else(|| match dirs::home_dir() {
            Some(mut path) => {
                path.push(".fs_cli_history");
                path
            }
            None => {
                warn!("HOME is unset, saving history in current directory");
                PathBuf::from(".fs_cli_history")
            }
        });

    if history_file.exists() {
        if let Err(e) = rl.load_history(&history_file) {
            warn!("Could not load history: {}", e);
        }
    }

    let prompt_host = if config.host == "localhost" {
        gethostname()
            .to_string_lossy()
            .to_string()
    } else {
        config
            .host
            .clone()
    };
    let prompt = format!("freeswitch@{}> ", prompt_host);

    loop {
        let result = if let Some(stashed) = rl.take_stashed_line() {
            rl.readline_with_initial(&prompt, (&stashed, ""))
        } else {
            rl.readline(&prompt)
        };

        match result {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                if let Err(e) = rl.add_history_entry(line) {
                    warn!("Could not add history entry: {}", e);
                }

                if matches!(line, "/quit" | "/exit" | "/bye") {
                    println!("Goodbye!");
                    let _ = quit_tx.send(());
                    break;
                }

                if line == "/history" {
                    println!("Command History:");
                    let history = rl.history();
                    let len = history.len();
                    for (i, entry) in history
                        .iter()
                        .enumerate()
                        .skip(len.saturating_sub(20))
                    {
                        println!("  {}: {}", i + 1, entry);
                    }
                    continue;
                }

                if cmd_tx
                    .send(line.to_string())
                    .is_err()
                {
                    break;
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

    if let Err(e) = rl.save_history(&history_file) {
        warn!("Could not save history: {}", e);
    }

    Ok(())
}
