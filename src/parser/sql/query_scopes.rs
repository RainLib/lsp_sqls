use super::types::SqlParser;
use tower_lsp::lsp_types::Position;

pub(super) struct CompletionScope {
    pub(super) source: String,
    pub(super) searchable: String,
}

impl SqlParser {
    /// Parenthesized joined inputs share their query block. Only actual query
    /// openings constrain source discovery in a closed sibling query.
    pub(crate) fn query_source_visible(searchable: &str, offset: usize, cursor: usize) -> bool {
        let Some(suffix) = searchable.get(offset..) else {
            return false;
        };
        // CTE discovery points at WITH itself, before the opening has seen its
        // first query token. Include that complete token when classifying it.
        let token = suffix
            .split(|c: char| !Self::is_identifier_char(c))
            .next()
            .unwrap_or("");
        let end = offset
            + if token.eq_ignore_ascii_case("SELECT") || token.eq_ignore_ascii_case("WITH") {
                token.len()
            } else {
                0
            };
        let prefix = &searchable[..end];
        query_openings(prefix).last().is_none_or(|open| {
            cursor > *open
                && cursor <= Self::matching_paren_end(searchable, *open).unwrap_or(searchable.len())
        })
    }
    /// Hide completed subquery clauses from completion heuristics while keeping
    /// every original byte/line coordinate and active function argument intact.
    pub(super) fn completion_context_source<'s>(
        source: &'s str,
        position: Position,
    ) -> std::borrow::Cow<'s, str> {
        use std::borrow::Cow;
        let cursor = Self::byte_offset_for_position(source, position).min(source.len());
        let prefix = &source[..cursor];
        if !prefix.contains('(') {
            return Cow::Borrowed(source);
        }
        let mask = Self::mask_sql_noise(prefix).to_ascii_uppercase();
        let regions = query_regions(&mask);
        let scope_start = regions.open.last().map_or_else(
            || Self::statement_boundary_in_searchable(&mask, None),
            |open| open + 1,
        );
        let ranges = regions.closed;
        if scope_start == 0 && ranges.is_empty() {
            return Cow::Borrowed(source);
        }
        let mut output = source.as_bytes().to_vec();
        for (start, end) in std::iter::once((0, scope_start)).chain(ranges) {
            for byte in &mut output[start..end] {
                if !matches!(*byte, b'\r' | b'\n') {
                    *byte = b' ';
                }
            }
        }
        Cow::Owned(String::from_utf8(output).expect("completion scope mask preserves UTF-8"))
    }

    pub(super) fn completion_scope_analysis(source: &str, position: Position) -> CompletionScope {
        let searchable = Self::mask_sql_noise(source);
        Self::completion_scope_with_mask(source, &searchable, position)
    }

    pub(super) fn completion_scope_with_mask(
        source: &str,
        searchable_source: &str,
        position: Position,
    ) -> CompletionScope {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let prefix = source
            .get(..cursor_offset.min(source.len()))
            .unwrap_or(source);
        let searchable_prefix = &searchable_source[..prefix.len()];
        let (scope_start, scope_open) = Self::innermost_query_scope_start(searchable_prefix);
        let scope_end = scope_open
            .and_then(|open| Self::matching_paren_end(searchable_source, open))
            .unwrap_or_else(|| Self::next_statement_end(searchable_source, cursor_offset));
        let range = if source
            .get(scope_start..scope_end.min(source.len()))
            .is_some()
        {
            scope_start..scope_end.min(source.len())
        } else {
            0..prefix.len()
        };
        Self::mask_nested_with_searchable(&source[range.clone()], &searchable_source[range])
    }

    pub(super) fn innermost_query_scope_start(searchable_prefix: &str) -> (usize, Option<usize>) {
        if let Some(open) = query_regions(searchable_prefix).open.last().copied() {
            return (open + 1, Some(open));
        }
        (
            Self::statement_boundary_in_searchable(&searchable_prefix.to_ascii_uppercase(), None),
            None,
        )
    }

    pub(super) fn matching_paren_end(searchable_source: &str, open_index: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (index, ch) in searchable_source[open_index..].char_indices() {
            let absolute = open_index + index;
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(absolute);
                    }
                }
                _ => {}
            }
        }

        None
    }

    pub(crate) fn next_statement_end(searchable_source: &str, cursor_offset: usize) -> usize {
        let mut cursor_offset = cursor_offset.min(searchable_source.len());
        while !searchable_source.is_char_boundary(cursor_offset) {
            cursor_offset = cursor_offset.saturating_sub(1);
        }
        let hard_end = searchable_source[cursor_offset..]
            .find(';')
            .map(|relative| cursor_offset + relative)
            .unwrap_or(searchable_source.len());
        Self::statement_boundary_in_searchable(&searchable_source[..hard_end], Some(cursor_offset))
    }

    pub(super) fn mask_nested_parenthesized_regions(source: &str) -> String {
        let searchable = Self::mask_sql_noise(source);
        Self::mask_nested_with_searchable(source, &searchable).source
    }

    pub(super) fn mask_nested_with_searchable(source: &str, searchable: &str) -> CompletionScope {
        let mut output = source.as_bytes().to_vec();
        let mut masked = searchable.as_bytes().to_vec();
        let mut depth = 0usize;
        for (index, byte) in searchable.bytes().enumerate() {
            let hide = match byte {
                b'(' => {
                    let hide = depth > 0;
                    depth += 1;
                    hide
                }
                b')' => {
                    depth = depth.saturating_sub(1);
                    depth > 0
                }
                _ => depth > 0,
            };
            if hide && !matches!(byte, b'\n' | b'\r') {
                output[index] = b' ';
                masked[index] = b' ';
            }
        }
        // Masking replaces complete nested byte ranges, retaining CR/LF and
        // all original byte offsets. Multi-byte identifiers outside them stay
        // intact; a hidden Unicode scalar is replaced by the same byte count.
        CompletionScope {
            source: String::from_utf8(output).expect("scope mask preserves UTF-8"),
            searchable: String::from_utf8(masked).expect("noise mask preserves UTF-8"),
        }
    }
}

struct QueryRegions {
    open: Vec<usize>,
    closed: Vec<(usize, usize)>,
}

fn query_regions(searchable: &str) -> QueryRegions {
    let mut stack: Vec<(usize, bool, bool)> = Vec::new();
    let mut closed: Vec<(usize, usize)> = Vec::new();
    let bytes = searchable.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => {
                if let Some((_, _, first)) = stack.last_mut() {
                    *first = false;
                }
                stack.push((index, false, true));
            }
            b')' => {
                if let Some((open, true, _)) = stack.pop() {
                    while closed.last().is_some_and(|(start, _)| *start > open) {
                        closed.pop();
                    }
                    closed.push((open + 1, index));
                }
            }
            b'A'..=b'Z' | b'a'..=b'z' => {
                let start = index;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'$'))
                {
                    index += 1;
                }
                let word = &searchable[start..index];
                let boundary = !searchable[..start]
                    .chars()
                    .next_back()
                    .is_some_and(SqlParser::is_identifier_char)
                    && !searchable[index..]
                        .chars()
                        .next()
                        .is_some_and(SqlParser::is_identifier_char);
                let mut following = searchable[index..].split_whitespace();
                let expansion = word.eq_ignore_ascii_case("WITH")
                    && following
                        .next()
                        .is_some_and(|word| word.eq_ignore_ascii_case("QUERY"))
                    && following
                        .next()
                        .is_some_and(|word| word.eq_ignore_ascii_case("EXPANSION"));
                if let Some((_, query, first)) = stack.last_mut() {
                    if *first
                        && boundary
                        && !expansion
                        && (word.eq_ignore_ascii_case("SELECT")
                            || word.eq_ignore_ascii_case("WITH"))
                    {
                        *query = true;
                    }
                    *first = false;
                }
                continue;
            }
            byte if !byte.is_ascii_whitespace() => {
                if let Some((_, _, first)) = stack.last_mut() {
                    *first = false;
                }
            }
            _ => {}
        }
        index += 1;
    }
    QueryRegions {
        open: stack
            .into_iter()
            .filter_map(|(index, query, _)| query.then_some(index))
            .collect(),
        closed,
    }
}

pub(super) fn query_openings(searchable: &str) -> Vec<usize> {
    query_regions(searchable).open
}
