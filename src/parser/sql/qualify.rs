//! Locate QUALIFY's own SELECT branch using the existing query/lexical masks.
//! This also works while its predicate is missing or has unclosed parentheses.
use super::types::SqlParser;
use std::ops::Range;

impl SqlParser {
    pub(crate) fn qualify_projection_range(
        source: &str,
        position: tower_lsp::lsp_types::Position,
    ) -> Option<Range<usize>> {
        let cursor = Self::byte_offset_for_position(source, position);
        let range = Self::qualify_query_range(source, cursor)?;
        let scoped = Self::completion_context_source(source, position);
        (!Self::predicate_continuation_context_at_position(&scoped, position)).then_some(range)
    }
    pub(crate) fn qualify_query_range(source: &str, cursor: usize) -> Option<Range<usize>> {
        if source.len() > 512 * 1024 || !source.is_char_boundary(cursor) {
            return None;
        }
        // Avoid an additional mask on the ordinary completion path.
        if !source
            .as_bytes()
            .windows(7)
            .any(|word| matches!(word[0], b'Q' | b'q') && word.eq_ignore_ascii_case(b"QUALIFY"))
        {
            return None;
        }
        let searchable = Self::mask_sql_noise(source);
        let (start, open) = Self::innermost_query_scope_start(&searchable[..cursor]);
        let end = open
            .and_then(|at| Self::matching_paren_end(&searchable, at))
            .unwrap_or_else(|| Self::next_statement_end(&searchable, cursor));
        let scoped =
            Self::mask_nested_with_searchable(&source[start..end], &searchable[start..end]);
        let relative = cursor.checked_sub(start)?;
        let mut branch = 0;
        let mut branch_end = scoped.searchable.len();
        let tokens: Vec<_> = scoped
            .searchable
            .match_indices(|ch: char| ch.is_alphabetic() || ch == '_')
            .filter(|(at, _)| {
                *at == 0
                    || !scoped.searchable[..*at]
                        .chars()
                        .next_back()
                        .is_some_and(Self::is_identifier_char)
            })
            .map(|(at, _)| {
                let word = scoped.searchable[at..]
                    .split(|ch: char| !Self::is_identifier_char(ch))
                    .next()
                    .unwrap_or("");
                (at, word)
            })
            .take(32769)
            .collect();
        if tokens.len() > 32768 {
            return None;
        }
        for &(at, word) in &tokens {
            if ["UNION", "EXCEPT", "INTERSECT"]
                .iter()
                .any(|keyword| word.eq_ignore_ascii_case(keyword))
            {
                if at < relative {
                    branch = at + word.len();
                } else {
                    branch_end = at;
                    break;
                }
            }
        }
        let select = tokens
            .iter()
            .find(|&&(at, word)| {
                at >= branch && at < relative && word.eq_ignore_ascii_case("SELECT")
            })?
            .0;
        let qualify = tokens
            .iter()
            .find(|&&(at, word)| {
                at > select && at < branch_end && word.eq_ignore_ascii_case("QUALIFY")
            })?
            .0;
        if relative < qualify + 7 || relative > branch_end {
            return None;
        }
        if tokens.iter().any(|&(at, word)| {
            at > qualify
                && at < relative
                && ["ORDER", "LIMIT", "OFFSET", "FETCH", "SETTINGS", "FORMAT"]
                    .iter()
                    .any(|keyword| word.eq_ignore_ascii_case(keyword))
        }) {
            return None;
        }
        Some(start + select..start + qualify)
    }
}
