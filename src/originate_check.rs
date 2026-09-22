//! What the switch will install for a typed `originate`.
//!
//! The switch consumes escapes in a dial string before installing anything, so a value written
//! with an apostrophe rarely arrives carrying one. This reads the line the way the switch does and
//! names the values the channel receives.

use freeswitch_esl_tokio::commands::{
    originate_split, DialStringCarrier, DialStringTarget, FlattenedDialString, Variables,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// What fs_cli does about an `originate` whose dial string the switch may read differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString, strum::Display, clap::ValueEnum)]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
#[clap(rename_all = "lowercase")]
pub enum OriginateCheck {
    /// Send the command as typed.
    Off,
    /// Report the values the channel will receive.
    Warn,
}

impl Serialize for OriginateCheck {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for OriginateCheck {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        raw.parse()
            .map_err(|_| {
                serde::de::Error::custom(format!(
                    "Invalid originate check: {}. Valid options: off, warn",
                    raw
                ))
            })
    }
}

/// What the switch will do with one command.
#[derive(Debug, PartialEq, Eq)]
pub struct Checked {
    /// What to tell the operator before the command goes out.
    pub report: Option<String>,
}

/// Read `command` as the switch reads it, under `policy`.
pub fn check(policy: OriginateCheck, command: &str) -> Checked {
    match policy {
        OriginateCheck::Off => Checked { report: None },
        OriginateCheck::Warn => Checked {
            report: report(command),
        },
    }
}

fn report(command: &str) -> Option<String> {
    let arguments = originate_arguments(command)?;
    let target = target_for(arguments);
    let tokens = match originate_split(arguments, ' ') {
        Ok(tokens) => tokens,
        Err(e) => return Some(format!("originate's argument split refuses this line: {e}")),
    };
    let dial = tokens.first()?;
    if !depth_sensitive(dial) && !opens_unclosed_block(dial) {
        return None;
    }
    let list = match FlattenedDialString::parse_for(dial, target) {
        Ok(list) => list,
        Err(e) => return Some(format!("the switch reads this dial string as {e}")),
    };

    let mut notes = delivered(dial, target);
    for warning in list.warnings() {
        notes.push(warning.to_string());
    }
    for leg in list.legs() {
        notes.extend(delivered(leg.raw(), target));
        for warning in leg.warnings() {
            notes.push(warning.to_string());
        }
    }
    (!notes.is_empty()).then(|| format!("the switch will set {}", notes.join(", ")))
}

/// The pairs each bracket block at the head of `text` installs, as the switch installs them.
fn delivered(text: &str, target: DialStringTarget) -> Vec<String> {
    let mut notes = Vec::new();
    for block in head_blocks(text) {
        match Variables::parse_for(block, target) {
            Ok(variables) => notes.extend(
                variables
                    .iter()
                    .map(|(key, value)| format!("{key}={value:?}")),
            ),
            Err(e) => notes.push(format!("nothing for one block, which reads as {e}")),
        }
    }
    notes
}

/// The `originate` arguments of `command`, past an `api` or `bgapi` prefix.
fn originate_arguments(command: &str) -> Option<&str> {
    let rest = command.trim_start();
    let rest = strip_word(rest, "bgapi")
        .or_else(|| strip_word(rest, "api"))
        .unwrap_or(rest);
    strip_word(rest, "originate")
}

/// `text` past a leading `word` and the blank after it.
fn strip_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let tail = text
        .get(..word.len())
        .filter(|head| head.eq_ignore_ascii_case(word))
        .map(|_| &text[word.len()..])?;
    tail.starts_with(|c: char| c.is_ascii_whitespace())
        .then(|| tail.trim_start())
}

/// Whether the dial string carries anything whose delivered value depends on escaping depth.
fn depth_sensitive(dial: &str) -> bool {
    dial.contains('\'') || dial.contains('\\') || dial.contains(":_:")
}

/// A block the argument split cut short, which an unescaped space in a value produces.
fn opens_unclosed_block(dial: &str) -> bool {
    dial.starts_with(['{', '<', '[']) && head_block(dial).is_none()
}

/// A line opening `^^` names the separator its arguments split on.
fn target_for(arguments: &str) -> DialStringTarget {
    let base = DialStringTarget::new(DialStringCarrier::EslApi);
    arguments
        .strip_prefix("^^")
        .and_then(|rest| {
            rest.chars()
                .next()
        })
        .and_then(|separator| {
            base.with_argv_separator(separator)
                .ok()
        })
        .unwrap_or(base)
}

/// The bracket blocks opening `text`, read as `switch_find_end_paren` reads them: depth counted,
/// no escape honoured.
fn head_blocks(text: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some((block, tail)) = head_block(rest) {
        blocks.push(block);
        rest = tail;
    }
    blocks
}

fn head_block(text: &str) -> Option<(&str, &str)> {
    let (open, close) = match text
        .as_bytes()
        .first()?
    {
        b'{' => (b'{', b'}'),
        b'<' => (b'<', b'>'),
        b'[' => (b'[', b']'),
        _ => return None,
    };
    let mut depth = 0usize;
    for (index, byte) in text
        .bytes()
        .enumerate()
    {
        if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(text.split_at(index + 1));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warned(command: &str) -> Option<String> {
        check(OriginateCheck::Warn, command).report
    }

    #[test]
    fn a_bare_quote_runs_the_rest_of_the_line_into_one_argument() {
        let report = warned("originate {cid_name=O'Brien}sofia/gw/x &park()").unwrap();
        assert!(report.contains("unclosed quote"), "{report}");
    }

    #[test]
    fn two_quotes_in_one_value_are_both_stripped() {
        let report = warned(r"originate {p1=l\'a\'b}sofia/gw/x &park()").unwrap();
        assert!(report.contains(r#"p1="lab""#), "{report}");
    }

    #[test]
    fn an_unescaped_space_truncates_the_dial_string() {
        let report = warned("originate {cid_name=O Brien}sofia/gw/x &park()").unwrap();
        assert!(report.contains("dial string"), "{report}");
    }

    #[test]
    fn a_bgapi_prefix_is_read_past() {
        assert!(warned(r"bgapi originate {p1=it\'s}sofia/gw/x &park()").is_some());
    }

    #[test]
    fn an_argument_separator_is_honoured() {
        let report = warned(r"originate ^^~{p1=it\'s~a b}loopback/9199/test~&park()");
        assert!(report.is_some());
    }

    #[test]
    fn an_unmodelled_endpoint_module_is_still_read() {
        let report = warned(r"originate {p1=it\'s}freetdm/1/0/5551212 &park()").unwrap();
        assert!(report.contains("p1="), "{report}");
    }

    #[test]
    fn a_dial_string_with_nothing_depth_sensitive_is_quiet() {
        assert_eq!(
            warned("originate {cid_name=Tremblay}sofia/gw/x &park()"),
            None
        );
    }

    #[test]
    fn a_command_that_is_not_an_originate_is_quiet() {
        assert_eq!(warned(r"uuid_setvar abc cid_name O\'Brien"), None);
        assert_eq!(warned("status"), None);
    }

    #[test]
    fn the_policy_switches_the_check_off() {
        assert_eq!(
            check(
                OriginateCheck::Off,
                r"originate {p1=it\'s,p2=SENTINEL}sofia/gw/x &park()"
            )
            .report,
            None
        );
    }
}
