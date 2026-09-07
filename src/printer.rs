//! Shared printer for coordinated terminal output.

use colored::{ColoredString, Colorize};
use rustyline::ExternalPrinter;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::{Arc, Mutex};
use tracing::warn;

/// Color mode for log display
#[derive(Debug, Clone, Copy, PartialEq, strum::EnumString, strum::Display)]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum ColorMode {
    Never,
    Tag,
    Line,
}

impl Serialize for ColorMode {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ColorMode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        raw.parse()
            .map_err(|_| {
                serde::de::Error::custom(format!(
                    "Invalid color mode: {}. Valid options: never, tag, line",
                    raw
                ))
            })
    }
}

/// Coordinated terminal printer. Clone is cheap (inner Arc clone).
#[derive(Clone)]
pub struct Printer(Option<Arc<Mutex<dyn ExternalPrinter + Send>>>);

impl Printer {
    /// Printer that falls back to stdout/stderr (no rustyline printer).
    pub fn none() -> Self {
        Self(None)
    }

    /// Printer backed by a rustyline ExternalPrinter.
    pub fn with_external(printer: impl ExternalPrinter + Send + 'static) -> Self {
        Self(Some(Arc::new(Mutex::new(printer))))
    }

    /// Print a message through the rustyline printer or stdout.
    pub fn print(&self, msg: String) {
        self.emit(msg, |m| println!("{}", m));
    }

    /// Print an error message through the rustyline printer or stderr.
    ///
    /// With a printer active every line goes through it, so the tty sees one
    /// redraw-safe path; the stderr split only exists in batch mode.
    pub fn print_err(&self, msg: String) {
        self.emit(msg, |m| eprintln!("{}", m));
    }

    /// Blocking lock: nothing holds it across an await, and `try_lock` used to
    /// bypass to raw stdout nondeterministically.
    fn emit(&self, msg: String, fallback: fn(&str)) {
        if let Some(arc) = &self.0 {
            match arc.lock() {
                Ok(mut p) => {
                    if let Err(e) = p.print(msg.clone()) {
                        warn!("ExternalPrinter::print failed ({:?}): {}", msg, e);
                        fallback(&msg);
                    }
                    return;
                }
                Err(e) => {
                    warn!("Printer mutex poisoned, falling back to stdio: {}", e);
                }
            }
        }
        fallback(&msg);
    }
}

/// Terminal output: where lines go and whether they carry color.
#[derive(Clone)]
pub struct Output {
    printer: Printer,
    color: ColorMode,
}

impl Output {
    pub fn new(color: ColorMode) -> Self {
        Self {
            printer: Printer::none(),
            color,
        }
    }

    pub fn set_printer(&mut self, printer: Printer) {
        self.printer = printer;
    }

    pub fn color(&self) -> ColorMode {
        self.color
    }

    pub fn print(&self, msg: String) {
        self.printer
            .print(msg);
    }

    pub fn print_err(&self, msg: String) {
        self.printer
            .print_err(msg);
    }

    /// Bold-red `Label: message`, with the error's full source chain.
    pub fn print_labeled_error(&self, label: &str, err: &anyhow::Error) {
        self.print_labeled(label, &format!("{:#}", err));
    }

    /// Bold-red `Label: message`.
    pub fn print_labeled(&self, label: &str, message: &str) {
        self.print_err(format!(
            "{}: {}",
            self.colorize(label, |s| s
                .red()
                .bold()),
            message
        ));
    }

    pub fn colorize(&self, text: &str, style: impl FnOnce(&str) -> ColoredString) -> String {
        match self.color {
            ColorMode::Never => text.to_string(),
            _ => style(text).to_string(),
        }
    }
}
