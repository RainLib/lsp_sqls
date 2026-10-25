//! Identifier path structure must survive normalization: a quoted dot is data.
use super::types::SqlParser;

pub(super) fn last_identifier_separator(input: &str) -> Option<usize> {
    let mut separator = None;
    let mut quote = None;
    let mut chars = input.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if let Some(close) = quote {
            if ch == close {
                if chars.peek().is_some_and(|(_, next)| *next == close) {
                    chars.next();
                } else {
                    quote = None;
                }
            }
        } else {
            match ch {
                '"' | '`' | '\'' => quote = Some(ch),
                '[' => quote = Some(']'),
                '.' => separator = Some(index),
                _ => {}
            }
        }
    }
    separator
}

pub(super) fn split_identifier_path(input: &str) -> Vec<String> {
    let input = input
        .trim()
        .trim_matches(|ch: char| matches!(ch, ',' | ';' | '(' | ')'));
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut chars = input.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        if let Some(close) = quote {
            if ch == close {
                if chars.peek().is_some_and(|(_, next)| *next == close) {
                    chars.next();
                } else {
                    quote = None;
                }
            }
        } else {
            match ch {
                '"' | '`' | '\'' => quote = Some(ch),
                '[' => quote = Some(']'),
                '.' => {
                    ranges.push(&input[start..at]);
                    start = at + 1;
                }
                _ => {}
            }
        }
    }
    ranges.push(&input[start..]);
    ranges
        .iter()
        .map(|part| decode_identifier_part(part))
        .collect()
}
fn decode_identifier_part(part: &str) -> String {
    let part = part.trim();
    let Some(first) = part.chars().next() else {
        return String::new();
    };
    let close = match first {
        '"' | '`' | '\'' => first,
        '[' => ']',
        _ => return part.to_string(),
    };
    let body = if part.len() >= 2 && part.ends_with(close) {
        &part[1..part.len() - 1]
    } else {
        &part[1..]
    };
    match close {
        '"' => body.replace("\"\"", "\""),
        '`' => body.replace("``", "`"),
        '\'' => body.replace("''", "'"),
        ']' => body.replace("]]", "]"),
        _ => unreachable!(),
    }
}
pub(super) fn encode_identifier_parts(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| {
            if part.is_empty() {
                return String::new();
            }
            let simple = part
                .chars()
                .next()
                .is_some_and(|ch| ch.is_alphabetic() || matches!(ch, '_' | '$' | '@' | '#'))
                && part.chars().all(SqlParser::is_identifier_char);
            if simple {
                part.clone()
            } else {
                format!("\"{}\"", part.replace('"', "\"\""))
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}
impl SqlParser {
    pub(crate) fn identifier_path_from_parts(parts: &[String]) -> String {
        encode_identifier_parts(parts)
    }
}
