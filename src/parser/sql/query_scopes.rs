use super::types::SqlParser;
use tower_lsp::lsp_types::Position;

impl SqlParser {
    pub(super) fn completion_scope_source(source: &str, position: Position) -> String {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let prefix = source
            .get(..cursor_offset.min(source.len()))
            .unwrap_or(source);
        let searchable_prefix = Self::mask_sql_noise(prefix);
        let searchable_source = Self::mask_sql_noise(source);
        let (scope_start, scope_open) = Self::innermost_query_scope_start(&searchable_prefix);
        let scope_end = scope_open
            .and_then(|open| Self::matching_paren_end(&searchable_source, open))
            .unwrap_or_else(|| Self::next_statement_end(&searchable_source, cursor_offset));
        let scoped = source
            .get(scope_start..scope_end.min(source.len()))
            .unwrap_or(prefix);

        Self::mask_nested_parenthesized_regions(scoped)
    }

    pub(super) fn innermost_query_scope_start(searchable_prefix: &str) -> (usize, Option<usize>) {
        let mut open_parens = Vec::new();
        for (index, ch) in searchable_prefix.char_indices() {
            match ch {
                '(' => open_parens.push(index),
                ')' => {
                    open_parens.pop();
                }
                _ => {}
            }
        }

        let upper = searchable_prefix.to_ascii_uppercase();
        for open in open_parens.into_iter().rev() {
            let start = open + 1;
            let segment = &upper[start..];
            if Self::next_keyword_position(segment, "SELECT", 0).is_some()
                || Self::next_keyword_position(segment, "WITH", 0).is_some()
            {
                return (start, Some(open));
            }
        }

        (Self::previous_statement_start(&upper, upper.len()), None)
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
        Self::statement_boundary(&searchable_source[..hard_end], Some(cursor_offset))
    }

    pub(super) fn mask_nested_parenthesized_regions(source: &str) -> String {
        let searchable = Self::mask_sql_noise(source);
        let mut output = String::with_capacity(source.len());
        let mut depth = 0usize;

        for (index, ch) in source.char_indices() {
            let searchable_ch = searchable[index..].chars().next().unwrap_or(ch);
            match searchable_ch {
                '(' => {
                    if depth == 0 {
                        output.push(ch);
                    } else {
                        Self::push_masked_char(&mut output, ch);
                    }
                    depth += 1;
                }
                ')' => {
                    if depth > 0 {
                        depth -= 1;
                        if depth == 0 {
                            output.push(ch);
                        } else {
                            Self::push_masked_char(&mut output, ch);
                        }
                    } else {
                        output.push(ch);
                    }
                }
                _ if depth > 0 => Self::push_masked_char(&mut output, ch),
                _ => output.push(ch),
            }
        }

        output
    }
}
