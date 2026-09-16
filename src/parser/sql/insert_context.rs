use super::types::SqlParser;

impl SqlParser {
    pub(super) fn is_insert_value_context(statement_upper: &str) -> bool {
        let Some(into_position) = Self::previous_keyword_position(statement_upper, "INSERT INTO")
        else {
            return false;
        };
        let Some(values_position) = Self::previous_keyword_position(statement_upper, "VALUES")
            .or_else(|| Self::previous_keyword_position(statement_upper, "VALUE"))
        else {
            return false;
        };
        if values_position < into_position {
            return false;
        }

        let after_values = values_position
            + if statement_upper[values_position..].starts_with("VALUES") {
                "VALUES".len()
            } else {
                "VALUE".len()
            };
        if Self::statement_has_any_keyword(
            statement_upper,
            after_values,
            statement_upper.len(),
            &["RETURNING", "ON CONFLICT", "ON DUPLICATE", "SELECT"],
        ) {
            return false;
        }

        let segment = &statement_upper[after_values..];
        let Some(last_open) = segment.rfind('(') else {
            return false;
        };
        let after_open = &segment[last_open + 1..];
        if after_open.contains(')') {
            return false;
        }

        let trimmed = after_open.trim_start();
        let trimmed_end = trimmed.trim_end();
        if trimmed_end.is_empty() || trimmed_end.ends_with(',') {
            return true;
        }

        let words = Self::statement_words(trimmed_end);
        if words.is_empty() || trimmed.chars().last().is_some_and(|ch| ch.is_whitespace()) {
            return false;
        }

        words
            .last()
            .is_some_and(|word| Self::is_value_keyword_prefix(word))
    }

    pub(super) fn is_insert_continuation_context(statement_upper: &str) -> bool {
        let searchable_statement_upper = Self::mask_sql_noise(statement_upper);
        let Some(into_position) =
            Self::previous_keyword_position(&searchable_statement_upper, "INSERT INTO")
        else {
            return false;
        };

        let Some((values_position, values_keyword)) =
            Self::previous_keyword_position(&searchable_statement_upper, "DEFAULT VALUES")
                .map(|position| (position, "DEFAULT VALUES"))
                .or_else(|| {
                    Self::previous_keyword_position(&searchable_statement_upper, "VALUES")
                        .map(|position| (position, "VALUES"))
                })
                .or_else(|| {
                    Self::previous_keyword_position(&searchable_statement_upper, "VALUE")
                        .map(|position| (position, "VALUE"))
                })
        else {
            return false;
        };
        if values_position < into_position {
            return false;
        }

        let after_values = values_position + values_keyword.len();
        if Self::statement_has_any_keyword(
            &searchable_statement_upper,
            after_values,
            searchable_statement_upper.len(),
            &["RETURNING", "ON CONFLICT", "ON DUPLICATE", "SELECT"],
        ) {
            return false;
        }

        let Some(segment) = statement_upper.get(after_values..) else {
            return false;
        };
        let trimmed = segment.trim_end();
        if trimmed.is_empty() {
            return values_keyword == "DEFAULT VALUES";
        }

        let continuation_tail = if values_keyword == "DEFAULT VALUES" {
            trimmed
        } else {
            let Some(last_close) = trimmed.rfind(')') else {
                return false;
            };
            &trimmed[last_close + 1..]
        };
        let prefix = continuation_tail.trim_start();

        prefix.is_empty() || Self::is_insert_continuation_prefix(&prefix.to_ascii_uppercase())
    }

    pub(super) fn is_insert_continuation_prefix(prefix: &str) -> bool {
        matches!(
            prefix,
            "O" | "ON"
                | "ON C"
                | "ON CO"
                | "ON CON"
                | "ON CONF"
                | "ON CONFL"
                | "ON CONFLI"
                | "ON CONFLIC"
                | "ON CONFLICT"
                | "ON D"
                | "ON DU"
                | "ON DUP"
                | "ON DUPL"
                | "ON DUPLI"
                | "ON DUPLIC"
                | "ON DUPLICA"
                | "ON DUPLICAT"
                | "ON DUPLICATE"
                | "R"
                | "RE"
                | "RET"
                | "RETU"
                | "RETUR"
                | "RETURN"
                | "RETURNI"
                | "RETURNIN"
                | "RETURNING"
        )
    }

    pub(super) fn latest_insert_conflict_segments<'raw, 'search>(
        raw_statement_upper: &'raw str,
        searchable_statement_upper: &'search str,
    ) -> Option<(&'raw str, &'search str)> {
        let into_position =
            Self::previous_keyword_position(searchable_statement_upper, "INSERT INTO")?;
        let conflict_position =
            Self::previous_keyword_position(searchable_statement_upper, "ON CONFLICT")?;
        if conflict_position < into_position {
            return None;
        }

        let after_conflict = conflict_position + "ON CONFLICT".len();
        if Self::statement_has_any_keyword(
            searchable_statement_upper,
            after_conflict,
            searchable_statement_upper.len(),
            &["RETURNING", "ON DUPLICATE"],
        ) {
            return None;
        }

        Some((
            raw_statement_upper.get(after_conflict..)?,
            searchable_statement_upper.get(after_conflict..)?,
        ))
    }

    pub(super) fn is_insert_conflict_target_context(statement_upper: &str) -> bool {
        let searchable_statement_upper = Self::mask_sql_noise(statement_upper);
        let Some((_, searchable_segment)) =
            Self::latest_insert_conflict_segments(statement_upper, &searchable_statement_upper)
        else {
            return false;
        };
        if Self::previous_keyword_position(searchable_segment, "DO").is_some() {
            return false;
        }

        let Some(last_open) = searchable_segment.rfind('(') else {
            return false;
        };
        !searchable_segment[last_open + 1..].contains(')')
    }

    pub(super) fn is_insert_conflict_constraint_context(statement_upper: &str) -> bool {
        let searchable_statement_upper = Self::mask_sql_noise(statement_upper);
        let Some((raw_segment, searchable_segment)) =
            Self::latest_insert_conflict_segments(statement_upper, &searchable_statement_upper)
        else {
            return false;
        };
        if Self::previous_keyword_position(searchable_segment, "DO").is_some() {
            return false;
        }

        let Some(on_constraint_position) =
            Self::previous_keyword_position(searchable_segment, "ON CONSTRAINT")
        else {
            return false;
        };
        let after_on_constraint = on_constraint_position + "ON CONSTRAINT".len();
        let Some(raw_tail) = raw_segment.get(after_on_constraint..) else {
            return false;
        };
        let trimmed = raw_tail.trim_start();
        let trimmed_end = trimmed.trim_end();
        if trimmed_end.is_empty() {
            return true;
        }

        !trimmed.chars().last().is_some_and(|ch| ch.is_whitespace())
            && Self::statement_words(trimmed_end).len() == 1
    }

    pub(super) fn is_insert_conflict_action_context(statement_upper: &str) -> bool {
        if Self::is_insert_conflict_target_context(statement_upper) {
            return false;
        }
        if Self::is_insert_conflict_constraint_context(statement_upper) {
            return false;
        }

        let searchable_statement_upper = Self::mask_sql_noise(statement_upper);
        let Some((raw_segment, searchable_segment)) =
            Self::latest_insert_conflict_segments(statement_upper, &searchable_statement_upper)
        else {
            return false;
        };
        if Self::statement_has_any_keyword(
            searchable_segment,
            0,
            searchable_segment.len(),
            &["DO NOTHING", "DO UPDATE"],
        ) {
            return false;
        }

        let tail_start = Self::insert_conflict_action_tail_start(searchable_segment);
        let Some(raw_tail) = raw_segment.get(tail_start..) else {
            return false;
        };
        let prefix = raw_tail.trim_start().trim_end().to_ascii_uppercase();

        prefix.is_empty() || Self::is_insert_conflict_action_prefix(&prefix)
    }

    pub(super) fn insert_conflict_action_tail_start(searchable_segment: &str) -> usize {
        let trimmed_start_len = searchable_segment.len() - searchable_segment.trim_start().len();
        let after_start = &searchable_segment[trimmed_start_len..];

        if after_start.starts_with('(') {
            if let Some(close_relative) = after_start.find(')') {
                return trimmed_start_len + close_relative + 1;
            }
        }
        if after_start.starts_with("ON CONSTRAINT") {
            let after_phrase = trimmed_start_len + "ON CONSTRAINT".len();
            let Some(tail) = searchable_segment.get(after_phrase..) else {
                return trimmed_start_len;
            };
            let leading_whitespace = tail.len() - tail.trim_start().len();
            let name_start = after_phrase + leading_whitespace;
            let Some(name_tail) = searchable_segment.get(name_start..) else {
                return trimmed_start_len;
            };
            let name_len = name_tail
                .chars()
                .take_while(|ch| Self::is_identifier_char(*ch))
                .map(char::len_utf8)
                .sum::<usize>();
            if name_len > 0 {
                return name_start + name_len;
            }
        }

        trimmed_start_len
    }

    pub(super) fn is_insert_conflict_action_prefix(prefix: &str) -> bool {
        [
            "(",
            "ON CONSTRAINT",
            "DO NOTHING",
            "DO UPDATE SET",
            "NOTHING",
            "UPDATE SET",
        ]
        .iter()
        .any(|keyword| keyword.starts_with(prefix))
    }

    pub(super) fn is_insert_duplicate_update_assignment_context(statement_upper: &str) -> bool {
        let Some(insert_position) = Self::previous_keyword_position(statement_upper, "INSERT INTO")
        else {
            return false;
        };
        let Some(duplicate_update_position) =
            Self::previous_keyword_position(statement_upper, "ON DUPLICATE KEY UPDATE")
        else {
            return false;
        };
        if duplicate_update_position < insert_position {
            return false;
        }

        let after_duplicate_update = duplicate_update_position + "ON DUPLICATE KEY UPDATE".len();
        let Some(tail) = statement_upper.get(after_duplicate_update..) else {
            return false;
        };
        let segment = tail.rsplit(',').next().unwrap_or(tail);
        let trimmed = segment.trim_end();
        if trimmed.trim_start().is_empty() {
            return true;
        }
        if Self::latest_value_operator_end(trimmed).is_some() {
            return false;
        }

        Self::statement_words(trimmed).len() <= 1
    }

    pub(super) fn is_insert_set_assignment_context(statement_upper: &str) -> bool {
        let Some(insert_position) = Self::previous_keyword_position(statement_upper, "INSERT INTO")
        else {
            return false;
        };
        let Some(set_position) = Self::previous_keyword_position(statement_upper, "SET") else {
            return false;
        };
        if set_position < insert_position {
            return false;
        }
        let after_insert = insert_position + "INSERT INTO".len();
        let Some(between_insert_and_set) = statement_upper.get(after_insert..set_position) else {
            return false;
        };
        if Self::statement_has_any_keyword(
            between_insert_and_set,
            0,
            between_insert_and_set.len(),
            &["VALUES", "VALUE", "SELECT", "UPDATE"],
        ) {
            return false;
        }

        let Some(tail) = statement_upper.get(set_position + "SET".len()..) else {
            return false;
        };
        let segment = tail.rsplit(',').next().unwrap_or(tail);
        let trimmed = segment.trim_end();
        if trimmed.trim_start().is_empty() {
            return true;
        }
        if Self::latest_value_operator_end(trimmed).is_some() {
            return false;
        }

        Self::statement_words(trimmed).len() <= 1
    }
}
