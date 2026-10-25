use super::soft_statements::{
    soft_statement_is_set_continuation, soft_statement_is_wrapped_continuation,
    soft_statement_keyword,
};
use super::types::SqlParser;

impl SqlParser {
    /// The final FROM in IS [NOT] DISTINCT FROM is an operator token, not a
    /// row-source introducer. Call with a quote/comment-masked source.
    pub(crate) fn is_distinct_from_keyword(source: &str, position: usize) -> bool {
        let Some(before) = source.get(..position) else {
            return false;
        };
        let mut words = before.split_whitespace().rev();
        if !words
            .next()
            .is_some_and(|word| word.eq_ignore_ascii_case("DISTINCT"))
        {
            return false;
        }
        match words.next() {
            Some(word) if word.eq_ignore_ascii_case("IS") => true,
            Some(word) if word.eq_ignore_ascii_case("NOT") => words
                .next()
                .is_some_and(|word| word.eq_ignore_ascii_case("IS")),
            _ => false,
        }
    }
    pub(super) fn is_keyword_at(source_upper: &str, start: usize, keyword: &str) -> bool {
        let end = start + keyword.len();
        if end > source_upper.len() {
            return false;
        }

        let before_is_boundary = if start == 0 {
            true
        } else {
            source_upper[..start]
                .chars()
                .next_back()
                .is_none_or(|ch| !Self::is_identifier_char(ch))
        };
        let after_is_boundary = if end >= source_upper.len() {
            true
        } else {
            source_upper[end..]
                .chars()
                .next()
                .is_none_or(|ch| !Self::is_identifier_char(ch))
        };

        before_is_boundary && after_is_boundary
    }

    pub(super) fn next_keyword_position(
        source_upper: &str,
        keyword: &str,
        from: usize,
    ) -> Option<usize> {
        let mut search_pos = from;
        while let Some(relative_pos) = source_upper[search_pos..].find(keyword) {
            let absolute_pos = search_pos + relative_pos;
            if Self::is_keyword_at(source_upper, absolute_pos, keyword)
                && !(keyword == "FROM"
                    && Self::is_distinct_from_keyword(source_upper, absolute_pos))
            {
                return Some(absolute_pos);
            }
            search_pos = absolute_pos + keyword.len();
        }
        None
    }

    pub(super) fn previous_keyword_position(source_upper: &str, keyword: &str) -> Option<usize> {
        let mut search_pos = 0;
        let mut previous = None;

        while let Some(position) = Self::next_keyword_position(source_upper, keyword, search_pos) {
            previous = Some(position);
            search_pos = position + keyword.len();
        }

        previous
    }

    /// Determines the active top-level statement for interactive editor
    /// contexts. Semicolons are authoritative; a blank line followed by a
    /// top-level statement keyword can start a new scope when users omit one.
    pub(crate) fn active_statement_start(source: &str) -> usize {
        Self::statement_boundary(source, None)
    }

    // With `after`, return the first soft boundary after the cursor. Both
    // directions use the same continuation and parenthesis rules.
    pub(super) fn statement_boundary(source: &str, after: Option<usize>) -> usize {
        let masked = Self::mask_sql_noise(source);
        Self::statement_boundary_in_searchable(&masked, after)
    }

    /// The caller already owns the byte-preserving noise mask. Re-masking it
    /// adds no information and is costly on every alias/navigation request.
    pub(super) fn statement_boundary_in_searchable(masked: &str, after: Option<usize>) -> usize {
        let prefix = &masked[..after.unwrap_or(masked.len()).min(masked.len())];
        let semicolon_start = prefix.rfind(';').map(|position| position + 1).unwrap_or(0);
        let Some(segment) = masked.get(semicolon_start..) else {
            return semicolon_start;
        };

        let mut initial_keyword: Option<&str> = None;
        let mut previous_non_empty_line = "";
        let mut blank_lines = 0usize;
        let mut parenthesis_depth = 0usize;
        let mut candidate_start = None;
        let mut line_offset = 0usize;

        for line_with_newline in segment.split_inclusive('\n') {
            let line = line_with_newline
                .strip_suffix('\n')
                .unwrap_or(line_with_newline);
            let trimmed = line.trim();
            let keyword = soft_statement_keyword(line);

            if initial_keyword.is_none() && !trimmed.is_empty() {
                initial_keyword = keyword;
            } else if blank_lines > 0 && parenthesis_depth == 0 {
                if let (Some(initial), Some(candidate)) = (initial_keyword, keyword) {
                    if !soft_statement_is_wrapped_continuation(initial, candidate)
                        && !soft_statement_is_set_continuation(previous_non_empty_line, candidate)
                    {
                        let leading = line.len() - line.trim_start().len();
                        let boundary = semicolon_start + line_offset + leading;
                        if after.is_some_and(|cursor| boundary > cursor) {
                            return boundary;
                        }
                        candidate_start = Some(boundary);
                        initial_keyword = Some(candidate);
                    }
                }
            }

            for character in line.chars() {
                match character {
                    '(' => parenthesis_depth += 1,
                    ')' => parenthesis_depth = parenthesis_depth.saturating_sub(1),
                    _ => {}
                }
            }
            if trimmed.is_empty() {
                if initial_keyword.is_some() {
                    blank_lines += 1;
                }
            } else {
                previous_non_empty_line = trimmed;
                blank_lines = 0;
            }
            line_offset += line_with_newline.len();
        }

        if after.is_some() {
            masked.len()
        } else {
            candidate_start.unwrap_or(semicolon_start)
        }
    }

    pub(crate) fn previous_statement_start(source_upper: &str, index: usize) -> usize {
        Self::active_statement_start(&source_upper[..index])
    }

    pub(super) fn contains_keyword_between(
        source_upper: &str,
        keyword: &str,
        start: usize,
        end: usize,
    ) -> bool {
        Self::next_keyword_position(source_upper, keyword, start)
            .is_some_and(|keyword_pos| keyword_pos < end)
    }

    pub(super) fn statement_has_any_keyword(
        source_upper: &str,
        start: usize,
        end: usize,
        keywords: &[&str],
    ) -> bool {
        keywords
            .iter()
            .any(|keyword| Self::contains_keyword_between(source_upper, keyword, start, end))
    }

    pub(super) fn should_read_on_relation(source_upper: &str, on_position: usize) -> bool {
        let statement_start = Self::previous_statement_start(source_upper, on_position);
        let has_ddl_action = Self::statement_has_any_keyword(
            source_upper,
            statement_start,
            on_position,
            &["CREATE", "DROP"],
        );
        let has_on_relation_object = Self::statement_has_any_keyword(
            source_upper,
            statement_start,
            on_position,
            &["INDEX", "TRIGGER", "POLICY"],
        );

        has_ddl_action && has_on_relation_object
    }

    pub(super) fn push_table_reference(tables: &mut Vec<String>, table_name: &str) {
        let table_name = Self::normalize_relation_reference(table_name);
        if !table_name.is_empty() && !tables.contains(&table_name) {
            tables.push(table_name);
        }
    }
}
