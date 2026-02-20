//! FreeSWITCH console_complete API integration

use crate::channel_info::ChannelProvider;
use crate::esl_debug::EslDebugLevel;
use freeswitch_esl_tokio::EslClient;

/// Get console completions from FreeSWITCH using the console_complete API
pub async fn get_console_complete(
    client: &EslClient,
    line: &str,
    pos: usize,
    debug_level: EslDebugLevel,
    channel_provider: &ChannelProvider,
) -> Vec<String> {
    let cmd = if pos > 0 && pos < line.len() {
        format!("console_complete c={};{}", pos, line)
    } else {
        format!("console_complete {}", line)
    };

    let is_uuid_command = line
        .trim_start()
        .starts_with("uuid_")
        && line.contains(' ');

    debug_level.debug_print(EslDebugLevel::Debug6, &format!("ESL API: {}", cmd));

    if is_uuid_command {
        if let Ok(Some(enhanced_completions)) = channel_provider
            .get_uuid_completions(client)
            .await
        {
            debug_level.debug_print(
                EslDebugLevel::Debug6,
                &format!(
                    "Using enhanced UUID completion with {} channels",
                    enhanced_completions.len()
                ),
            );
            return enhanced_completions;
        }
        debug_level.debug_print(
            EslDebugLevel::Debug6,
            "Falling back to default UUID completion",
        );
    }

    match client
        .api(&cmd)
        .await
    {
        Ok(response) => {
            debug_level.debug_print(
                EslDebugLevel::Debug6,
                &format!("ESL Response success: {}", response.is_success()),
            );

            if let Some(body) = response.body() {
                debug_level.debug_print(
                    EslDebugLevel::Debug6,
                    &format!("ESL Response body (escaped): {:?}", body),
                );
                debug_level.debug_print(
                    EslDebugLevel::Debug6,
                    &format!("ESL Response body (raw):\n---START---\n{}\n---END---", body),
                );
                let parsed_completions = parse_console_complete_response(body);
                debug_level.debug_print(
                    EslDebugLevel::Debug6,
                    &format!("Parsed completions: {:?}", parsed_completions),
                );
                parsed_completions
            } else {
                debug_level.debug_print(
                    EslDebugLevel::Debug6,
                    &format!("ESL Response: no body for command: {}", cmd),
                );
                Vec::new()
            }
        }
        Err(e) => {
            tracing::debug!("Failed to get console completions: {}", e);
            Vec::new()
        }
    }
}

/// Parse the console_complete response from FreeSWITCH
pub fn parse_console_complete_response(body: &str) -> Vec<String> {
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
                    completions.push(option_text.to_string());
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
                completions.push(format!("WRITE_DIRECTIVE:{}", replacement_text));
            }
        }
    }

    completions
}
