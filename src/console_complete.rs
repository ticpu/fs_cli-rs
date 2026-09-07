//! FreeSWITCH console_complete API integration

use crate::channel_info::ChannelProvider;
use crate::completion::{Completion, CompletionRequest};
use freeswitch_esl_tokio::EslClient;
use tracing::{trace, warn};

/// Get console completions from FreeSWITCH using the console_complete API
pub async fn get_console_complete(
    client: &EslClient,
    request: &CompletionRequest,
    channel_provider: &ChannelProvider,
) -> Vec<Completion> {
    let line = request
        .line
        .as_str();
    let pos = request.pos;

    let cmd = if pos > 0 && pos < line.len() {
        format!("console_complete c={};{}", pos, line)
    } else {
        format!("console_complete {}", line)
    };

    let is_uuid_command = line
        .trim_start()
        .starts_with("uuid_")
        && line.contains(' ');

    trace!("ESL API: {}", cmd);

    if is_uuid_command {
        match channel_provider
            .get_uuid_completions(client)
            .await
        {
            Ok(Some(enhanced_completions)) => {
                trace!(
                    "Using enhanced UUID completion with {} channels",
                    enhanced_completions.len()
                );
                return enhanced_completions;
            }
            Ok(None) => trace!("Falling back to default UUID completion"),
            Err(e) => {
                warn!("UUID channel lookup failed, falling back: {:#}", e);
            }
        }
    }

    match client
        .api(&cmd)
        .await
    {
        Ok(response) => match response.api_result() {
            Ok(body) => {
                trace!("ESL Response body (escaped): {:?}", body);
                let parsed = parse_console_complete_response(body);
                trace!("Parsed completions: {:?}", parsed);
                parsed
            }
            Err(e) => {
                trace!("No completions for '{}': {}", cmd, e);
                Vec::new()
            }
        },
        Err(e) => {
            tracing::debug!("Failed to get console completions: {}", e);
            Vec::new()
        }
    }
}

/// Parse the console_complete response from FreeSWITCH
pub fn parse_console_complete_response(body: &str) -> Vec<Completion> {
    let mut completions = Vec::new();

    for line in body.lines() {
        let mut chars = line
            .chars()
            .peekable();
        while let Some(ch) = chars.next() {
            if ch == '[' {
                let mut bracket_content = String::new();
                for inner_ch in chars.by_ref() {
                    if inner_ch == ']' {
                        break;
                    }
                    bracket_content.push(inner_ch);
                }

                let option_text = bracket_content.trim();
                if !option_text.is_empty() {
                    completions.push(Completion::Candidate(option_text.to_string()));
                }
            }
        }
    }

    if !completions.is_empty() {
        return completions;
    }

    if let Some(write_start) = body.find("write=") {
        let write_section = &body[write_start + 6..];
        if let Some(colon_pos) = write_section.find(':') {
            let replacement_text = write_section[colon_pos + 1..].trim_end();
            if !replacement_text.is_empty() {
                completions.push(Completion::Write(replacement_text.to_string()));
            }
        }
    }

    completions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracket_list_yields_one_candidate_per_bracket() {
        let completions = parse_console_complete_response("[status]\t[reload]\t[shutdown]");
        assert_eq!(completions.len(), 3);
        assert!(matches!(&completions[0], Completion::Candidate(s) if s == "status"));
        assert!(matches!(&completions[1], Completion::Candidate(s) if s == "reload"));
        assert!(matches!(&completions[2], Completion::Candidate(s) if s == "shutdown"));
    }

    #[test]
    fn write_fallback_yields_write() {
        let completions = parse_console_complete_response("write=5:status profile");
        assert_eq!(completions.len(), 1);
        assert!(matches!(&completions[0], Completion::Write(s) if s == "status profile"));
    }

    #[test]
    fn empty_body_yields_nothing() {
        assert!(parse_console_complete_response("").is_empty());
    }

    #[test]
    fn brackets_win_over_write_in_same_body() {
        let completions = parse_console_complete_response("[status]\nwrite=5:status profile");
        assert_eq!(completions.len(), 1);
        assert!(matches!(&completions[0], Completion::Candidate(s) if s == "status"));
    }
}
