//! Tab completion support for fs_cli-rs

use crate::esl_debug::EslDebugLevel;
use crate::CompletionRequest;
use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::highlight::{CmdKind, Highlighter, MatchingBracketHighlighter};
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::validate::{self, MatchingBracketValidator, Validator};
use rustyline::{Context, Helper};
use std::borrow::Cow::{self, Borrowed, Owned};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Standard UUID length in characters (8-4-4-4-12 format)
const UUID_LEN: usize = 36;

/// Find the longest common prefix among a list of strings
fn find_common_prefix(strings: &[&str]) -> String {
    if strings.is_empty() {
        return String::new();
    }

    if strings.len() == 1 {
        return strings[0].to_string();
    }

    let first = strings[0];
    let mut prefix_len = 0;

    for (i, ch) in first.chars().enumerate() {
        if strings.iter().all(|s| s.chars().nth(i) == Some(ch)) {
            prefix_len = i + 1;
        } else {
            break;
        }
    }

    first.chars().take(prefix_len).collect()
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

        // Find the current word being completed
        let line_bytes = line.as_bytes();
        let mut start = pos;

        // Find start of current word (go back to last space or start)
        while start > 0 && line_bytes[start - 1] != b' ' {
            start -= 1;
        }

        let current_word = &line[start..pos];

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
    fn get_esl_completions(&self, line: &str, pos: usize) -> Vec<String> {
        self.debug_level.debug_print(
            EslDebugLevel::Debug6,
            &format!("get_esl_completions called for '{}' pos {}", line, pos),
        );

        if let Some(completion_tx) = &self.completion_tx {
            self.debug_level
                .debug_print(EslDebugLevel::Debug6, "Have completion channel");

            // Create a channel to receive the response
            let (response_tx, response_rx) = oneshot::channel();

            // Send completion request to main thread
            let request = CompletionRequest {
                line: line.to_string(),
                pos,
                response_tx,
            };

            if completion_tx.send(request).is_err() {
                self.debug_level
                    .debug_print(EslDebugLevel::Debug6, "Failed to send completion request");
                return Vec::new();
            }

            self.debug_level.debug_print(
                EslDebugLevel::Debug6,
                "Sent completion request, waiting for response...",
            );

            // Wait for response with timeout (blocking call from sync context)
            // We use a thread spawn to handle async within sync context
            match std::thread::spawn(move || {
                // Create a new runtime for this thread
                let rt = tokio::runtime::Runtime::new().ok()?;
                rt.block_on(async {
                    tokio::time::timeout(Duration::from_millis(500), response_rx)
                        .await
                        .ok()?
                        .ok()
                })
            })
            .join()
            {
                Ok(Some(completions)) => {
                    self.debug_level.debug_print(
                        EslDebugLevel::Debug6,
                        &format!("Received completions: {:?}", completions),
                    );
                    completions
                }
                Ok(None) => {
                    self.debug_level
                        .debug_print(EslDebugLevel::Debug6, "Received None from response");
                    Vec::new()
                }
                Err(e) => {
                    self.debug_level.debug_print(
                        EslDebugLevel::Debug6,
                        &format!("Thread join error: {:?}", e),
                    );
                    Vec::new()
                }
            }
        } else {
            self.debug_level
                .debug_print(EslDebugLevel::Debug6, "No completion channel available");
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
        if !line.trim_start().starts_with('/') {
            // Try ESL completion first for FreeSWITCH commands
            let esl_completions = self.get_esl_completions(line, pos);

            if !esl_completions.is_empty() {
                // Convert ESL completions to Pair format
                let mut candidates = Vec::new();

                // Find the current word being completed
                let line_bytes = line.as_bytes();
                let mut start = pos;
                while start > 0 && line_bytes[start - 1] != b' ' {
                    start -= 1;
                }
                let current_word = &line[start..pos];

                for completion in esl_completions {
                    // Handle write= directive specially
                    if let Some(replacement_text) = completion.strip_prefix("WRITE_DIRECTIVE:") {
                        // Skip "WRITE_DIRECTIVE:"
                        candidates.push(Pair {
                            display: replacement_text.to_string(),
                            replacement: replacement_text.to_string(),
                        });
                    } else if completion.len() > UUID_LEN
                        && completion.chars().nth(UUID_LEN) == Some(' ')
                        && completion
                            .chars()
                            .take(UUID_LEN)
                            .all(|c| c.is_ascii_hexdigit() || c == '-')
                    {
                        // This looks like UUID completion format: "uuid timestamp name (state)"
                        // Extract just the UUID (first UUID_LEN characters) for replacement
                        let uuid = &completion[..UUID_LEN];
                        if uuid.starts_with(current_word) {
                            candidates.push(Pair {
                                display: completion.clone(),
                                replacement: format!("{} ", uuid),
                            });
                        }
                    } else if completion.starts_with(current_word) {
                        // Return the full completion as replacement since rustyline
                        // will replace from start position, not append at current position
                        candidates.push(Pair {
                            display: completion.clone(),
                            replacement: completion.clone(),
                        });
                    }
                }

                // Handle single vs multiple candidates differently
                if candidates.len() == 1 {
                    // Single candidate - add trailing space like C readline
                    let candidate = &mut candidates[0];
                    if !candidate.replacement.ends_with(' ') {
                        candidate.replacement.push(' ');
                    }
                } else if candidates.len() > 1 {
                    // Multiple candidates - need to calculate common prefix and adjust replacements
                    let completions: Vec<&str> =
                        candidates.iter().map(|c| c.display.as_str()).collect();
                    let common_prefix = find_common_prefix(&completions);

                    if common_prefix.len() > current_word.len() {
                        // There's a common prefix beyond what user typed - complete to it
                        for candidate in &mut candidates {
                            candidate.replacement = common_prefix.clone();
                        }
                    } else {
                        // No useful common prefix - return each full completion for list display
                        // Keep the full replacements as they are for proper list display
                    }
                }

                if !candidates.is_empty() {
                    return Ok((start, candidates));
                }
            }
        }

        // Fallback to static command completion
        let (start, candidates) = self.complete_command(line, pos)?;

        // If no command matches and we're completing a path-like string, try filename completion
        if candidates.is_empty() && (line.contains('/') || line.contains('\\')) {
            let (file_start, file_candidates) = self.filename_completer.complete(line, pos, ctx)?;
            return Ok((file_start, file_candidates));
        }

        Ok((start, candidates))
    }
}

impl Hinter for FsCliCompleter {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<Self::Hint> {
        self.history_hinter.hint(line, pos, ctx)
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
        self.bracket_highlighter.highlight(line, pos)
    }

    fn highlight_char(&self, line: &str, pos: usize, kind: CmdKind) -> bool {
        self.bracket_highlighter.highlight_char(line, pos, kind)
    }
}

impl Validator for FsCliCompleter {
    fn validate(
        &self,
        ctx: &mut validate::ValidationContext,
    ) -> rustyline::Result<validate::ValidationResult> {
        self.bracket_validator.validate(ctx)
    }

    fn validate_while_typing(&self) -> bool {
        self.bracket_validator.validate_while_typing()
    }
}
