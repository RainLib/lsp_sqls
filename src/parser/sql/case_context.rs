use super::types::SqlParser;

impl SqlParser {
    pub(super) fn latest_open_case_segments<'raw, 'search>(
        raw_statement_upper: &'raw str,
        searchable_statement_upper: &'search str,
    ) -> Option<(&'raw str, &'search str)> {
        let case_position = Self::previous_keyword_position(searchable_statement_upper, "CASE")?;
        let after_case = case_position + "CASE".len();
        if Self::statement_has_any_keyword(
            searchable_statement_upper,
            after_case,
            searchable_statement_upper.len(),
            &["END"],
        ) {
            return None;
        }

        Some((
            raw_statement_upper.get(after_case..)?,
            searchable_statement_upper.get(after_case..)?,
        ))
    }

    pub(super) fn latest_case_result_marker(
        searchable_segment_upper: &str,
    ) -> Option<(usize, &'static str)> {
        ["THEN", "ELSE"]
            .into_iter()
            .filter_map(|marker| {
                Self::previous_keyword_position(searchable_segment_upper, marker)
                    .map(|position| (position, marker))
            })
            .max_by_key(|(position, _)| *position)
    }

    pub(super) fn simple_case_has_base_expression(searchable_case_segment: &str) -> bool {
        let Some(first_when_position) =
            Self::next_keyword_position(searchable_case_segment, "WHEN", 0)
        else {
            return false;
        };

        !searchable_case_segment[..first_when_position]
            .trim()
            .is_empty()
    }

    pub(super) fn latest_simple_case_when_value_segment(statement_upper: &str) -> Option<&str> {
        let searchable_statement_upper = Self::mask_sql_noise(statement_upper);
        let (raw_case_segment, searchable_case_segment) =
            Self::latest_open_case_segments(statement_upper, &searchable_statement_upper)?;
        if !Self::simple_case_has_base_expression(searchable_case_segment) {
            return None;
        }

        let when_position = Self::previous_keyword_position(searchable_case_segment, "WHEN")?;
        let after_when = when_position + "WHEN".len();
        if Self::statement_has_any_keyword(
            searchable_case_segment,
            after_when,
            searchable_case_segment.len(),
            &["THEN"],
        ) {
            return None;
        }

        raw_case_segment.get(after_when..)
    }

    pub(super) fn is_simple_case_when_value_context(statement_upper: &str) -> bool {
        let Some(value_segment) = Self::latest_simple_case_when_value_segment(statement_upper)
        else {
            return false;
        };
        let trimmed = value_segment.trim_end();
        if trimmed.is_empty() || Self::case_result_needs_more_expression(trimmed) {
            return true;
        }
        if value_segment
            .chars()
            .last()
            .is_some_and(|ch| ch.is_whitespace())
        {
            return false;
        }

        let words = Self::statement_words(trimmed);
        let Some(prefix) = words.last().copied() else {
            return false;
        };
        let Some(prefix_start) = trimmed.rfind(prefix) else {
            return false;
        };

        trimmed[..prefix_start].trim_end().is_empty()
    }

    pub(super) fn is_simple_case_when_value_continuation_context(statement_upper: &str) -> bool {
        let Some(value_segment) = Self::latest_simple_case_when_value_segment(statement_upper)
        else {
            return false;
        };
        let trimmed = value_segment.trim_end();
        if trimmed.is_empty() || Self::case_result_needs_more_expression(trimmed) {
            return false;
        }

        if value_segment
            .chars()
            .last()
            .is_some_and(|ch| ch.is_whitespace())
        {
            return Self::is_completed_case_result_expression(trimmed);
        }

        let words = Self::statement_words(trimmed);
        let Some(prefix) = words.last().copied() else {
            return false;
        };
        if !matches!(prefix, "T" | "TH" | "THE" | "THEN") {
            return false;
        }
        let Some(prefix_start) = trimmed.rfind(prefix) else {
            return false;
        };
        let before_prefix = trimmed[..prefix_start].trim_end();

        !before_prefix.is_empty() && Self::is_completed_case_result_expression(before_prefix)
    }

    pub(super) fn is_case_result_context(statement_upper: &str) -> bool {
        let searchable_statement_upper = Self::mask_sql_noise(statement_upper);
        let Some((raw_case_segment, searchable_case_segment)) =
            Self::latest_open_case_segments(statement_upper, &searchable_statement_upper)
        else {
            return false;
        };
        let Some((marker_position, marker)) =
            Self::latest_case_result_marker(searchable_case_segment)
        else {
            return false;
        };
        let after_marker = marker_position + marker.len();
        let Some(result_segment) = raw_case_segment.get(after_marker..) else {
            return false;
        };
        let trimmed = result_segment.trim_end();
        if trimmed.is_empty() || Self::case_result_needs_more_expression(trimmed) {
            return true;
        }
        if result_segment
            .chars()
            .last()
            .is_some_and(|ch| ch.is_whitespace())
        {
            return false;
        }

        let words = Self::statement_words(trimmed);
        let Some(prefix) = words.last().copied() else {
            return false;
        };
        let Some(prefix_start) = trimmed.rfind(prefix) else {
            return false;
        };

        trimmed[..prefix_start].trim_end().is_empty()
    }

    pub(super) fn is_case_continuation_context(statement_upper: &str) -> bool {
        let searchable_statement_upper = Self::mask_sql_noise(statement_upper);
        let Some((raw_case_segment, searchable_case_segment)) =
            Self::latest_open_case_segments(statement_upper, &searchable_statement_upper)
        else {
            return false;
        };
        let Some((marker_position, marker)) =
            Self::latest_case_result_marker(searchable_case_segment)
        else {
            return false;
        };
        let after_marker = marker_position + marker.len();
        let Some(result_segment) = raw_case_segment.get(after_marker..) else {
            return false;
        };
        let trimmed = result_segment.trim_end();
        if trimmed.is_empty() || Self::case_result_needs_more_expression(trimmed) {
            return false;
        }

        if result_segment
            .chars()
            .last()
            .is_some_and(|ch| ch.is_whitespace())
        {
            return Self::is_completed_case_result_expression(trimmed);
        }

        let words = Self::statement_words(trimmed);
        let Some(prefix) = words.last().copied() else {
            return false;
        };
        if !Self::is_case_continuation_prefix(prefix, marker) {
            return false;
        }
        let Some(prefix_start) = trimmed.rfind(prefix) else {
            return false;
        };
        let before_prefix = trimmed[..prefix_start].trim_end();

        !before_prefix.is_empty() && Self::is_completed_case_result_expression(before_prefix)
    }

    pub(super) fn case_result_needs_more_expression(segment: &str) -> bool {
        segment
            .chars()
            .last()
            .is_some_and(|ch| matches!(ch, '(' | ',' | '.' | '+' | '-' | '*' | '/' | '%' | '='))
    }

    pub(super) fn is_completed_case_result_expression(segment: &str) -> bool {
        let value = segment.trim();
        if value.is_empty() || Self::case_result_needs_more_expression(value) {
            return false;
        }
        if Self::is_completed_value_expression(value) {
            return true;
        }

        let token = value
            .split(|ch: char| ch.is_whitespace() || matches!(ch, '(' | ')' | ',' | ';'))
            .rfind(|token| !token.is_empty())
            .unwrap_or("")
            .trim_matches(|ch| matches!(ch, '"' | '\'' | '`' | '[' | ']'));
        if token.is_empty() {
            return false;
        }

        !matches!(
            token.to_ascii_uppercase().as_str(),
            "CASE" | "WHEN" | "THEN" | "ELSE" | "END"
        )
    }

    pub(super) fn is_case_continuation_prefix(prefix: &str, marker: &str) -> bool {
        matches!(
            (marker, prefix),
            ("THEN", "W" | "WH" | "WHE" | "WHEN")
                | ("THEN", "E" | "EL" | "ELS" | "ELSE" | "EN" | "END")
                | ("ELSE", "E" | "EN" | "END")
        )
    }

    pub(super) fn is_value_keyword_prefix(word: &str) -> bool {
        matches!(
            word,
            "D" | "DE"
                | "DEF"
                | "DEFA"
                | "DEFAU"
                | "DEFAUL"
                | "DEFAULT"
                | "N"
                | "NO"
                | "NOW"
                | "NU"
                | "NUL"
                | "NULL"
                | "T"
                | "TR"
                | "TRU"
                | "TRUE"
                | "F"
                | "FA"
                | "FAL"
                | "FALS"
                | "FALSE"
                | "TO"
                | "TOD"
                | "TODA"
                | "TODAY"
                | "C"
                | "CU"
                | "CUR"
                | "CURR"
                | "CURRE"
                | "CURREN"
                | "CURRENT"
                | "CURRENT_DATE"
                | "CURRENT_TIMESTAMP"
        )
    }
}
