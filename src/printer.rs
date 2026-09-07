//! Shared printer for coordinated terminal output.

use colored::{ColoredString, Colorize};
use rustyline::ExternalPrinter;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::warn;

/// Color mode for log display
#[derive(Debug, Clone, Copy, PartialEq, strum::EnumString, strum::Display, clap::ValueEnum)]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
#[clap(rename_all = "lowercase")]
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
        self.emit(msg, &|m| println!("{}", m));
    }

    /// Print an error message through the rustyline printer or stderr.
    ///
    /// With a printer active every line goes through it, so the tty sees one
    /// redraw-safe path; the stderr split only exists in batch mode.
    pub fn print_err(&self, msg: String) {
        self.emit(msg, &|m| eprintln!("{}", m));
    }

    /// Blocking lock: nothing holds it across an await, and `try_lock` used to
    /// bypass to raw stdout nondeterministically.
    fn emit(&self, msg: String, fallback: &dyn Fn(&str)) {
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

/// A file or stdout, written line by line as an `ExternalPrinter` so it can
/// back a [`Printer`] like rustyline's own.
pub struct LogSink {
    writer: Box<dyn Write + Send>,
    broken: Arc<AtomicBool>,
}

impl LogSink {
    /// Sink writing to an already-opened file.
    pub fn file(file: std::fs::File) -> Self {
        Self::new(Box::new(file))
    }

    /// Sink writing to this process's stdout.
    pub fn stdout() -> Self {
        Self::new(Box::new(std::io::stdout()))
    }

    fn new(writer: Box<dyn Write + Send>) -> Self {
        Self {
            writer,
            broken: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Flag the caller keeps once the sink is handed to a [`Printer`].
    pub fn broken_flag(&self) -> Arc<AtomicBool> {
        self.broken
            .clone()
    }
}

impl ExternalPrinter for LogSink {
    /// A failure is recorded rather than returned: `Printer` answers an `Err`
    /// by printing to stdout, which for a closed pipe panics.
    fn print(&mut self, msg: String) -> rustyline::Result<()> {
        if self
            .broken
            .load(Ordering::Relaxed)
        {
            return Ok(());
        }
        if let Err(e) = writeln!(self.writer, "{}", msg).and_then(|()| {
            self.writer
                .flush()
        }) {
            warn!("log destination write failed, stopping log capture: {}", e);
            self.broken
                .store(true, Ordering::Relaxed);
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use rustyline::error::ReadlineError;
    use std::cell::Cell;

    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<String>>>);

    impl Recorder {
        fn lines(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .clone()
        }
    }

    impl ExternalPrinter for Recorder {
        fn print(&mut self, msg: String) -> rustyline::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(msg);
            Ok(())
        }
    }

    struct BrokenPrinter;

    impl ExternalPrinter for BrokenPrinter {
        fn print(&mut self, _msg: String) -> rustyline::Result<()> {
            Err(ReadlineError::Io(std::io::Error::other(
                "external printer is gone",
            )))
        }
    }

    fn output_with(color: ColorMode, recorder: &Recorder) -> Output {
        let mut output = Output::new(color);
        output.set_printer(Printer::with_external(recorder.clone()));
        output
    }

    #[test]
    fn both_streams_go_through_the_external_printer() {
        let recorder = Recorder::default();
        let printer = Printer::with_external(recorder.clone());
        printer.print("out".to_string());
        printer.print_err("err".to_string());
        assert_eq!(recorder.lines(), vec!["out", "err"]);
    }

    #[test]
    fn without_a_printer_the_line_takes_the_stdio_fallback() {
        let taken = Mutex::new(Vec::new());
        Printer::none().emit("plain".to_string(), &|m| {
            taken
                .lock()
                .unwrap()
                .push(m.to_string())
        });
        assert_eq!(
            *taken
                .lock()
                .unwrap(),
            vec!["plain"]
        );
    }

    #[test]
    fn a_failing_printer_still_delivers_the_line() {
        let taken = Mutex::new(Vec::new());
        Printer::with_external(BrokenPrinter).emit("plain".to_string(), &|m| {
            taken
                .lock()
                .unwrap()
                .push(m.to_string())
        });
        assert_eq!(
            *taken
                .lock()
                .unwrap(),
            vec!["plain"]
        );
    }

    #[test]
    fn print_labeled_joins_label_and_message() {
        let recorder = Recorder::default();
        output_with(ColorMode::Never, &recorder).print_labeled("Warning", "disk is full");
        assert_eq!(recorder.lines(), vec!["Warning: disk is full"]);
    }

    #[test]
    fn print_labeled_error_keeps_the_whole_source_chain() {
        let recorder = Recorder::default();
        let err = anyhow::anyhow!("connection refused").context("could not reach FreeSWITCH");
        output_with(ColorMode::Never, &recorder).print_labeled_error("Error", &err);
        assert_eq!(
            recorder.lines(),
            vec!["Error: could not reach FreeSWITCH: connection refused"]
        );
    }

    #[test]
    fn colorize_never_returns_the_text_untouched() {
        let styled = Cell::new(false);
        let plain = Output::new(ColorMode::Never).colorize("tag", |s| {
            styled.set(true);
            ColoredString::from(s)
        });
        assert_eq!(plain, "tag");
        assert!(!styled.get());
    }

    #[test]
    fn colorize_applies_the_style_when_color_is_on() {
        for mode in [ColorMode::Tag, ColorMode::Line] {
            let styled = Output::new(mode).colorize("tag", |_| ColoredString::from("styled"));
            assert_eq!(styled, "styled");
        }
    }
}
