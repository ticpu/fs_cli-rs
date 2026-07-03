//! Tab completion support for fs_cli-rs

use crate::console_complete::Completion;
use crate::esl_debug::EslDebugLevel;
use crate::readline::CompletionRequest;
use rustyline::completion::{
    extract_word, longest_common_prefix, Completer, FilenameCompleter, Pair,
};
use rustyline::highlight::{CmdKind, Highlighter, MatchingBracketHighlighter};
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::validate::{self, MatchingBracketValidator, Validator};
use rustyline::{Context, Helper};
use std::borrow::Cow::{self, Borrowed, Owned};
use std::time::Duration;
use tokio::sync::mpsc;

/// Add a trailing space to the single candidate's replacement if not already present.
/// No-op when the slice is empty or has more than one element.
fn add_trailing_space(candidates: &mut [Pair]) {
    if let [candidate] = candidates {
        if !candidate
            .replacement
            .ends_with(' ')
        {
            candidate
                .replacement
                .push(' ');
        }
    }
}

/// FreeSWITCH CLI completer with command suggestions
pub struct FsCliCompleter {
    filename_completer: FilenameCompleter,
    history_hinter: HistoryHinter,
    bracket_highlighter: MatchingBracketHighlighter,
    bracket_validator: MatchingBracketValidator,
    completion_tx: Option<mpsc::UnboundedSender<CompletionRequest>>,
    debug_level: EslDebugLevel,
}

impl FsCliCompleter {
    /// Create new completer
    pub fn new() -> Self {
        Self {
            filename_completer: FilenameCompleter::new(),
            history_hinter: HistoryHinter::new(),
            bracket_highlighter: MatchingBracketHighlighter::new(),
            bracket_validator: MatchingBracketValidator::new(),
            completion_tx: None,
            debug_level: EslDebugLevel::None,
        }
    }

    /// Create new completer with completion channel for ESL-based completions
    pub fn new_with_completion_channel(
        completion_tx: mpsc::UnboundedSender<CompletionRequest>,
        debug_level: EslDebugLevel,
    ) -> Self {
        Self {
            filename_completer: FilenameCompleter::new(),
            history_hinter: HistoryHinter::new(),
            bracket_highlighter: MatchingBracketHighlighter::new(),
            bracket_validator: MatchingBracketValidator::new(),
            completion_tx: Some(completion_tx),
            debug_level,
        }
    }

    /// Get FreeSWITCH command suggestions
    fn get_fs_commands() -> Vec<&'static str> {
        vec![
            // Basic commands
            "status",
            "version",
            "uptime",
            "help",
            // Show commands
            "show",
            "show channels",
            "show channels count",
            "show calls",
            "show registrations",
            "show modules",
            "show interfaces",
            "show api",
            "show application",
            "show codec",
            "show file",
            "show timer",
            "show tasks",
            "show complete",
            // Control commands
            "reload",
            "reloadxml",
            "reload mod_sofia",
            "reload mod_dialplan_xml",
            "originate",
            // Sofia commands
            "sofia",
            "sofia status",
            "sofia profile",
            "sofia profile internal",
            "sofia profile external",
            "sofia global",
            // Channel commands
            "uuid_answer",
            "uuid_hangup",
            "uuid_transfer",
            "uuid_bridge",
            "uuid_park",
            "uuid_hold",
            "uuid_break",
            "uuid_kill",
            // Conference commands
            "conference",
            "conference list",
            "conference kick",
            "conference mute",
            "conference unmute",
            // System commands
            "fsctl",
            "fsctl pause",
            "fsctl resume",
            "fsctl shutdown",
            "fsctl crash",
            "fsctl send_sighup",
            "load",
            "unload",
            "bgapi",
            // Log commands
            "console",
            "log",
            "uuid_dump",
            // Database commands
            "db",
            "group",
            "user_exists",
            // Other common commands
            "hupall",
            "pause",
            "resume",
            "shutdown",
            "expr",
            "eval",
            "expand",
            "global_getvar",
            "global_setvar",
        ]
    }

    /// Get command completions for a given input
    fn complete_command(&self, line: &str, pos: usize) -> rustyline::Result<(usize, Vec<Pair>)> {
        let commands = Self::get_fs_commands();
        let (start, current_word) = extract_word(line, pos, None, |c| c == ' ');

        // Find matching commands
        let matches: Vec<Pair> = commands
            .into_iter()
            .filter(|cmd| {
                // For multi-word commands, check if they start with current line
                if cmd.starts_with(&line[..start]) {
                    // Get the next word in the command after current position
                    let remaining = &cmd[start..];
                    if let Some(next_space) = remaining.find(' ') {
                        let next_word = &remaining[..next_space];
                        next_word.starts_with(current_word)
                    } else {
                        remaining.starts_with(current_word)
                    }
                } else {
                    // Single word commands
                    start == 0 && cmd.starts_with(current_word)
                }
            })
            .map(|cmd| {
                // Extract just the word we're completing
                let remaining = &cmd[start..];
                let next_word = if let Some(space_pos) = remaining.find(' ') {
                    &remaining[..space_pos]
                } else {
                    remaining
                };

                Pair {
                    display: next_word.to_string(),
                    replacement: next_word[current_word.len()..].to_string(),
                }
            })
            .collect();

        Ok((pos, matches))
    }

    /// Get ESL-based completions from FreeSWITCH
    fn get_esl_completions(&self, line: &str, pos: usize) -> Vec<Completion> {
        self.debug_level
            .debug_print(EslDebugLevel::Debug6, || {
                format!("get_esl_completions called for '{}' pos {}", line, pos)
            });

        if let Some(completion_tx) = &self.completion_tx {
            self.debug_level
                .debug_print(EslDebugLevel::Debug6, || {
                    "Have completion channel".to_string()
                });

            let (response_tx, response_rx) = std::sync::mpsc::sync_channel::<Vec<Completion>>(1);

            let request = CompletionRequest {
                line: line.to_string(),
                pos,
                response_tx,
            };

            if completion_tx
                .send(request)
                .is_err()
            {
                self.debug_level
                    .debug_print(EslDebugLevel::Debug6, || {
                        "Failed to send completion request".to_string()
                    });
                return Vec::new();
            }

            self.debug_level
                .debug_print(EslDebugLevel::Debug6, || {
                    "Sent completion request, waiting for response...".to_string()
                });

            match response_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(completions) => {
                    self.debug_level
                        .debug_print(EslDebugLevel::Debug6, || {
                            format!("Received {} completions", completions.len())
                        });
                    completions
                }
                Err(e) => {
                    self.debug_level
                        .debug_print(EslDebugLevel::Debug6, || {
                            format!("Completion response error: {}", e)
                        });
                    Vec::new()
                }
            }
        } else {
            self.debug_level
                .debug_print(EslDebugLevel::Debug6, || {
                    "No completion channel available".to_string()
                });
            Vec::new()
        }
    }
}

impl Default for FsCliCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl Helper for FsCliCompleter {}

impl Completer for FsCliCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        // Skip ESL completion for client-side commands (starting with /)
        if !line
            .trim_start()
            .starts_with('/')
        {
            // Try ESL completion first for FreeSWITCH commands
            let esl_completions = self.get_esl_completions(line, pos);

            if !esl_completions.is_empty() {
                let mut candidates = Vec::new();
                let (start, current_word) = extract_word(line, pos, None, |c| c == ' ');

                for completion in esl_completions {
                    match completion {
                        Completion::Write(text) => {
                            candidates.push(Pair {
                                display: text.clone(),
                                replacement: text,
                            });
                        }
                        Completion::Uuid { uuid, display } => {
                            if uuid.starts_with(current_word) {
                                candidates.push(Pair {
                                    display,
                                    replacement: format!("{} ", uuid),
                                });
                            }
                        }
                        Completion::Candidate(s) => {
                            if s.starts_with(current_word) {
                                candidates.push(Pair {
                                    display: s.clone(),
                                    replacement: s,
                                });
                            }
                        }
                    }
                }

                if candidates.len() == 1 {
                    add_trailing_space(&mut candidates);
                } else if candidates.len() > 1 {
                    // Compute LCP of replacement values; if it extends beyond what the
                    // user already typed, complete to it so multiple matches narrow down.
                    // rustyline's longest_common_prefix uses replacement(), not display(),
                    // giving a cleaner boundary on UUID completions.
                    let lcp = longest_common_prefix(&candidates).map(|s| s.to_string());
                    if let Some(lcp) = lcp {
                        if lcp.len() > current_word.len() {
                            for candidate in &mut candidates {
                                candidate.replacement = lcp.clone();
                            }
                        }
                    }
                }

                if !candidates.is_empty() {
                    return Ok((start, candidates));
                }
            }
        }

        // Fallback to static command completion
        let (start, mut candidates) = self.complete_command(line, pos)?;
        add_trailing_space(&mut candidates);

        // If no command matches and we're completing a path-like string, try filename completion
        if candidates.is_empty() && (line.contains('/') || line.contains('\\')) {
            let (file_start, file_candidates) = self
                .filename_completer
                .complete(line, pos, ctx)?;
            return Ok((file_start, file_candidates));
        }

        Ok((start, candidates))
    }
}

impl Hinter for FsCliCompleter {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<Self::Hint> {
        self.history_hinter
            .hint(line, pos, ctx)
    }
}

impl Highlighter for FsCliCompleter {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        default: bool,
    ) -> Cow<'b, str> {
        if default {
            Borrowed(prompt)
        } else {
            Owned(format!("\x1b[1m{}\x1b[0m", prompt)) // Bold prompt when not default
        }
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Owned(format!("\x1b[90m{}\x1b[0m", hint)) // Gray hint
    }

    fn highlight<'l>(&self, line: &'l str, pos: usize) -> Cow<'l, str> {
        self.bracket_highlighter
            .highlight(line, pos)
    }

    fn highlight_char(&self, line: &str, pos: usize, kind: CmdKind) -> bool {
        self.bracket_highlighter
            .highlight_char(line, pos, kind)
    }
}

impl Validator for FsCliCompleter {
    fn validate(
        &self,
        ctx: &mut validate::ValidationContext,
    ) -> rustyline::Result<validate::ValidationResult> {
        self.bracket_validator
            .validate(ctx)
    }

    fn validate_while_typing(&self) -> bool {
        self.bracket_validator
            .validate_while_typing()
    }
}
