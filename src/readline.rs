//! Readline thread and function key management

use crate::completion::FsCliCompleter;
use crate::config::AppConfig;
use anyhow::Result;
use gethostname::gethostname;
use rustyline::history::FileHistory;
use rustyline::{Cmd, Editor, ExternalPrinter, KeyCode, KeyEvent, Modifiers};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{error, warn};

/// Completion request from readline thread to main thread
#[derive(Debug)]
pub struct CompletionRequest {
    pub line: String,
    pub pos: usize,
    pub response_tx: std::sync::mpsc::SyncSender<Vec<String>>,
}

/// Default FreeSWITCH function key bindings
pub fn get_default_fnkeys() -> HashMap<String, String> {
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
            rl.bind_sequence(f_key, Cmd::MacroClearLine(format!("{}\n", command)));
        }
    }
    Ok(())
}

/// Run the readline loop in a blocking thread
pub fn run_readline_loop(
    cmd_tx: mpsc::UnboundedSender<String>,
    quit_tx: oneshot::Sender<()>,
    printer_tx: oneshot::Sender<Arc<Mutex<dyn ExternalPrinter + Send>>>,
    completion_tx: mpsc::UnboundedSender<CompletionRequest>,
    config: &AppConfig,
) -> Result<()> {
    let rl_config = rustyline::Config::builder()
        .completion_type(rustyline::CompletionType::List)
        .completion_show_all_if_ambiguous(true)
        .build();
    let mut rl = Editor::<FsCliCompleter, FileHistory>::with_config(rl_config)?;

    let completer = FsCliCompleter::new_with_completion_channel(completion_tx, config.debug);
    rl.set_helper(Some(completer));

    let macros = build_macros(config);
    setup_function_key_bindings(&mut rl, &macros)?;

    let printer = rl.create_external_printer()?;
    let printer_arc = Arc::new(Mutex::new(printer));
    let _ = printer_tx.send(printer_arc);

    let history_file = config
        .history_file
        .clone()
        .unwrap_or_else(|| {
            let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            path.push(".fs_cli_history");
            path
        });

    if history_file.exists() {
        if let Err(e) = rl.load_history(&history_file) {
            warn!("Could not load history: {}", e);
        }
    }

    loop {
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

        let result = if let Some(restore_content) = rl.take_pending_restore() {
            rl.readline_with_initial(&prompt, (&restore_content, ""))
        } else {
            rl.readline(&prompt)
        };

        match result {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(line);

                if matches!(line, "/quit" | "/exit" | "/bye") {
                    println!("Goodbye!");
                    let _ = quit_tx.send(());
                    break;
                }

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
