//! Readline thread and function key management

use crate::client_command::ClientCommand;
use crate::completion::{CompletionRequest, FsCliCompleter};
use crate::config::AppConfig;
use crate::printer::Printer;
use anyhow::Result;
use gethostname::gethostname;
use rustyline::history::{FileHistory, History};
use rustyline::{Cmd, Editor, EventHandler, KeyCode, KeyEvent, Modifiers, Movement};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, warn};

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

/// Everything the readline thread sends on, plus the macros it binds.
pub struct ReadlineChannels {
    commands: mpsc::UnboundedSender<String>,
    quit: oneshot::Sender<()>,
    printer: oneshot::Sender<Printer>,
    completions: mpsc::UnboundedSender<CompletionRequest>,
    macros: HashMap<String, String>,
}

/// The session-side ends of `ReadlineChannels`.
pub struct SessionChannels {
    pub commands: mpsc::UnboundedReceiver<String>,
    pub quit: oneshot::Receiver<()>,
    pub printer: oneshot::Receiver<Printer>,
    pub completions: mpsc::UnboundedReceiver<CompletionRequest>,
}

impl ReadlineChannels {
    pub fn new(macros: HashMap<String, String>) -> (Self, SessionChannels) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (quit_tx, quit_rx) = oneshot::channel();
        let (printer_tx, printer_rx) = oneshot::channel();
        let (completion_tx, completion_rx) = mpsc::unbounded_channel();
        (
            Self {
                commands: cmd_tx,
                quit: quit_tx,
                printer: printer_tx,
                completions: completion_tx,
                macros,
            },
            SessionChannels {
                commands: cmd_rx,
                quit: quit_rx,
                printer: printer_rx,
                completions: completion_rx,
            },
        )
    }
}

/// The F1-F12 keys that have a command, in key order.
pub fn fn_key_bindings(macros: &HashMap<String, String>) -> impl Iterator<Item = (u8, &String)> {
    (1u8..=12).filter_map(|i| {
        macros
            .get(&format!("f{}", i))
            .map(|command| (i, command))
    })
}

fn setup_function_key_bindings(
    rl: &mut Editor<FsCliCompleter, FileHistory>,
    macros: &HashMap<String, String>,
) -> Result<()> {
    for (i, command) in fn_key_bindings(macros) {
        rl.bind_sequence(
            KeyEvent(KeyCode::F(i), Modifiers::NONE),
            EventHandler::Macro(vec![
                Cmd::Stash,
                Cmd::Kill(Movement::WholeLine),
                Cmd::Insert(1, command.clone()),
                Cmd::AcceptLine,
            ]),
        );
    }
    Ok(())
}

/// How many trailing history entries `/history` prints.
const HISTORY_DISPLAY_COUNT: usize = 20;

fn print_history(rl: &Editor<FsCliCompleter, FileHistory>) {
    println!("Command History:");
    let history = rl.history();
    let len = history.len();
    for (i, entry) in history
        .iter()
        .enumerate()
        .skip(len.saturating_sub(HISTORY_DISPLAY_COUNT))
    {
        println!("  {}: {}", i + 1, entry);
    }
}

/// Build the editor with its completer and F-key macros bound.
fn build_editor(
    completion_tx: mpsc::UnboundedSender<CompletionRequest>,
    macros: &HashMap<String, String>,
) -> Result<Editor<FsCliCompleter, FileHistory>> {
    let rl_config = rustyline::Config::builder()
        .completion_type(rustyline::CompletionType::List)
        .completion_show_all_if_ambiguous(true)
        .build();
    let mut rl = Editor::<FsCliCompleter, FileHistory>::with_config(rl_config)?;
    rl.set_helper(Some(FsCliCompleter::new(completion_tx)));
    setup_function_key_bindings(&mut rl, macros)?;
    Ok(rl)
}

/// Where history is loaded from and saved to.
pub fn resolve_history_file(config: &AppConfig) -> PathBuf {
    config
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
        })
}

/// The prompt shown before every line.
pub fn build_prompt(config: &AppConfig) -> String {
    let host = if config.host == "localhost" {
        gethostname()
            .to_string_lossy()
            .to_string()
    } else {
        config
            .host
            .clone()
    };
    format!("freeswitch@{}> ", host)
}

/// What one entered line asks the readline thread to do.
#[derive(Debug, PartialEq, Eq)]
pub enum LineOutcome {
    Ignore,
    ShowHistory,
    Quit,
    Send(String),
}

/// Both `/quit` and `/history` are handled here rather than by the session:
/// this thread owns the history and the quit signal.
pub fn classify_line(line: &str) -> LineOutcome {
    let line = line.trim();
    if line.is_empty() {
        return LineOutcome::Ignore;
    }
    match line.parse::<ClientCommand>() {
        Ok(ClientCommand::Quit) => LineOutcome::Quit,
        Ok(ClientCommand::History) => LineOutcome::ShowHistory,
        _ => LineOutcome::Send(line.to_string()),
    }
}

/// Why the read/dispatch loop stopped.
enum ReadlineExit {
    /// The user asked to leave; the session still needs the quit signal.
    Quit,
    /// The session is already gone or input is unusable.
    Detached,
}

fn read_dispatch_loop(
    rl: &mut Editor<FsCliCompleter, FileHistory>,
    prompt: &str,
    cmd_tx: &mpsc::UnboundedSender<String>,
) -> ReadlineExit {
    loop {
        let result = if let Some(stashed) = rl.take_stashed_line() {
            rl.readline_with_initial(prompt, (&stashed, ""))
        } else {
            rl.readline(prompt)
        };

        match result {
            Ok(line) => {
                let outcome = classify_line(&line);
                if outcome != LineOutcome::Ignore {
                    if let Err(e) = rl.add_history_entry(line.trim()) {
                        warn!("Could not add history entry: {}", e);
                    }
                }
                match outcome {
                    LineOutcome::Ignore => continue,
                    LineOutcome::ShowHistory => print_history(rl),
                    LineOutcome::Quit => {
                        println!("Goodbye!");
                        return ReadlineExit::Quit;
                    }
                    LineOutcome::Send(command) => {
                        if cmd_tx
                            .send(command)
                            .is_err()
                        {
                            return ReadlineExit::Detached;
                        }
                    }
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => println!("^C"),
            Err(rustyline::error::ReadlineError::Eof) => {
                println!("Goodbye!");
                return ReadlineExit::Quit;
            }
            Err(e) => {
                error!("Error reading input: {}", e);
                return ReadlineExit::Detached;
            }
        }
    }
}

/// Run the readline loop in a blocking thread
pub fn run_readline_loop(chans: ReadlineChannels, config: &AppConfig) -> Result<()> {
    let ReadlineChannels {
        commands: cmd_tx,
        quit: quit_tx,
        printer: printer_tx,
        completions: completion_tx,
        macros,
    } = chans;

    let mut rl = build_editor(completion_tx, &macros)?;

    let printer = rl.create_external_printer()?;
    if printer_tx
        .send(Printer::with_external(printer))
        .is_err()
    {
        warn!("Session ended before printer was delivered");
    }

    let history_file = resolve_history_file(config);
    if history_file.exists() {
        if let Err(e) = rl.load_history(&history_file) {
            warn!("Could not load history: {}", e);
        }
    }

    let prompt = build_prompt(config);

    if let ReadlineExit::Quit = read_dispatch_loop(&mut rl, &prompt, &cmd_tx) {
        if quit_tx
            .send(())
            .is_err()
        {
            warn!("Quit signal lost, session will not be told to exit");
        }
    }

    if let Err(e) = rl.save_history(&history_file) {
        warn!("Could not save history: {}", e);
    }

    Ok(())
}
