//! Shared printer for coordinated terminal output.

use rustyline::ExternalPrinter;
use std::sync::{Arc, Mutex};
use tracing::warn;

/// Coordinated terminal printer. Clone is cheap (inner Arc clone).
///
/// Uses `std::sync::Mutex` with a blocking lock: nothing holds this lock
/// across an await point, contention is momentary, and blocking eliminates
/// the nondeterministic raw-stdout bypass that `try_lock` caused.
///
/// When no printer is present (non-interactive or batch mode) output goes
/// to stdout/stderr directly.
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
        if let Some(arc) = &self.0 {
            match arc.lock() {
                Ok(mut p) => {
                    if let Err(e) = p.print(msg.clone()) {
                        warn!("ExternalPrinter::print failed ({:?}): {}", msg, e);
                        println!("{}", msg);
                    }
                    return;
                }
                Err(e) => {
                    warn!("Printer mutex poisoned, falling back to stdout: {}", e);
                }
            }
        }
        println!("{}", msg);
    }

    /// Print an error message through the rustyline printer or stderr.
    ///
    /// When a printer is active (interactive session), errors go through it so
    /// all output reaches the tty via the same redraw-safe path. stderr split
    /// only applies when there is no printer (batch / non-interactive mode).
    pub fn print_err(&self, msg: String) {
        if let Some(arc) = &self.0 {
            match arc.lock() {
                Ok(mut p) => {
                    if let Err(e) = p.print(msg.clone()) {
                        warn!("ExternalPrinter::print failed ({:?}): {}", msg, e);
                        eprintln!("{}", msg);
                    }
                    return;
                }
                Err(e) => {
                    warn!("Printer mutex poisoned, falling back to stderr: {}", e);
                }
            }
        }
        eprintln!("{}", msg);
    }
}
