//! Bounded-by-input, byte-preserving recognition of MySQL's compound full-text
//! expression. Parenthesis pairs are indexed once; nested calls are skipped.
use super::types::SqlParser;
use crate::placeholder::SqlPlaceholderDialect;
use std::collections::HashMap;

pub(crate) struct MatchAgainst {
    pub start: usize,
    pub end: usize,
    column_close: usize,
    query_open: usize,
    mode_start: Option<usize>,
}

impl MatchAgainst {
    pub(super) fn adapt(&self, bytes: &mut [u8]) {
        bytes[self.start..self.start + 5].copy_from_slice(b"m_f_t");
        bytes[self.column_close] = b',';
        blank(&mut bytes[self.column_close + 1..self.query_open + 1]);
        if let Some(start) = self.mode_start {
            blank(&mut bytes[start..self.end - 1]);
        }
    }
}

fn blank(bytes: &mut [u8]) {
    for byte in bytes {
        if !matches!(*byte, b'\r' | b'\n') {
            *byte = b' ';
        }
    }
}

impl SqlParser {
    pub(crate) fn match_against_expressions(source: &str) -> Vec<MatchAgainst> {
        if !source
            .as_bytes()
            .windows(5)
            .any(|word| word.eq_ignore_ascii_case(b"MATCH"))
        {
            return Vec::new();
        }
        let mask = Self::mask_sql_operand_context(source, SqlPlaceholderDialect::Mysql)
            .to_ascii_uppercase();
        Self::match_against_in_mask(source, &mask)
    }

    pub(super) fn match_against_in_mask(source: &str, mask: &str) -> Vec<MatchAgainst> {
        let mut stack = Vec::new();
        let mut pairs = HashMap::new();
        for (index, byte) in mask.bytes().enumerate() {
            if byte == b'(' {
                stack.push(index);
            } else if byte == b')' {
                if let Some(open) = stack.pop() {
                    pairs.insert(open, index);
                }
            }
        }
        let mut found = Vec::new();
        for (start, _) in mask.match_indices("MATCH") {
            let before = mask[..start].trim_end().chars().next_back();
            let adjacent = source[..start].chars().next_back();
            if matches!(before, Some('.' | ':'))
                || adjacent.is_some_and(Self::is_identifier_char)
                || source[start + 5..]
                    .chars()
                    .next()
                    .is_some_and(Self::is_identifier_char)
            {
                continue;
            }
            let open = Self::skip_whitespace(mask, start + 5);
            let Some(close) = pairs.get(&open).copied() else {
                continue;
            };
            if !Self::fulltext_columns(source, mask, open + 1, close) {
                continue;
            }
            let against = Self::skip_whitespace(mask, close + 1);
            if !mask[against..].starts_with("AGAINST")
                || mask[against + 7..]
                    .chars()
                    .next()
                    .is_some_and(Self::is_identifier_char)
            {
                continue;
            }
            let query_open = Self::skip_whitespace(mask, against + 7);
            let Some(query_close) = pairs.get(&query_open).copied() else {
                continue;
            };
            let mut index = query_open + 1;
            let mut mode_start = None;
            let mut invalid = false;
            while index < query_close {
                if let Some(close) = pairs.get(&index) {
                    index = close + 1;
                    continue;
                }
                if mask.as_bytes()[index] == b',' {
                    invalid = true;
                    break;
                }
                if matches!(mask.as_bytes()[index], b'I' | b'W')
                    && Self::mode_at(mask, index, query_close)
                {
                    mode_start = Some(index);
                    break;
                }
                index += 1;
            }
            let query_end = mode_start.unwrap_or(query_close);
            if invalid || Self::skip_whitespace(mask, query_open + 1) >= query_end {
                continue;
            }
            found.push(MatchAgainst {
                start,
                end: query_close + 1,
                column_close: close,
                query_open,
                mode_start,
            });
        }
        found
    }

    fn fulltext_columns(source: &str, mask: &str, start: usize, end: usize) -> bool {
        let mut index = Self::skip_whitespace(mask, start);
        if index >= end {
            return false;
        }
        loop {
            if source.as_bytes().get(index) == Some(&b'\'') {
                return false;
            }
            let Some((_, next)) = Self::read_identifier_part(source, index) else {
                return false;
            };
            index = Self::skip_whitespace(mask, next);
            while mask.as_bytes().get(index) == Some(&b'.') {
                index = Self::skip_whitespace(mask, index + 1);
                let Some((_, next)) = Self::read_identifier_part(source, index) else {
                    return false;
                };
                index = Self::skip_whitespace(mask, next);
            }
            if index == end {
                return true;
            }
            if mask.as_bytes().get(index) != Some(&b',') {
                return false;
            }
            index = Self::skip_whitespace(mask, index + 1);
            if index >= end {
                return false;
            }
        }
    }

    fn mode_at(mask: &str, start: usize, end: usize) -> bool {
        if mask[..start]
            .chars()
            .next_back()
            .is_some_and(Self::is_identifier_char)
        {
            return false;
        }
        let words: Vec<_> = mask[start..end].split_whitespace().take(8).collect();
        matches!(
            words.as_slice(),
            ["IN", "BOOLEAN", "MODE"]
                | ["IN", "NATURAL", "LANGUAGE", "MODE"]
                | ["WITH", "QUERY", "EXPANSION"]
                | [
                    "IN",
                    "NATURAL",
                    "LANGUAGE",
                    "MODE",
                    "WITH",
                    "QUERY",
                    "EXPANSION"
                ]
        )
    }
}
