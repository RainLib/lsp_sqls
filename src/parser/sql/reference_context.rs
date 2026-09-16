use super::types::SqlParser;
use tower_lsp::lsp_types::Position;

impl SqlParser {
    pub(super) fn is_reference_relation_target_context(statement_upper: &str) -> bool {
        let Some(references_position) =
            Self::previous_keyword_position(statement_upper, "REFERENCES")
        else {
            return false;
        };
        let after_references = references_position + "REFERENCES".len();
        if Self::statement_has_any_keyword(
            statement_upper,
            after_references,
            statement_upper.len(),
            &[
                "MATCH",
                "ON DELETE",
                "ON UPDATE",
                "DEFERRABLE",
                "NOT DEFERRABLE",
                "INITIALLY",
            ],
        ) {
            return false;
        }

        if statement_upper[after_references..].contains('(') {
            return false;
        }

        let text_after_references = &statement_upper[after_references..];
        if text_after_references.trim_start().is_empty() {
            return true;
        }

        let Some((_, after_relation)) = Self::read_relation_reference_after_preserving_trailing(
            statement_upper,
            after_references,
        ) else {
            return true;
        };
        let remainder = &statement_upper[after_relation..];

        if remainder.is_empty() {
            return true;
        }
        if remainder.trim().is_empty() {
            return false;
        }

        remainder.trim_start().starts_with('.')
    }

    pub(super) fn is_reference_action_context(statement_upper: &str) -> bool {
        let Some(references_position) =
            Self::previous_keyword_position(statement_upper, "REFERENCES")
        else {
            return false;
        };
        let after_references = references_position + "REFERENCES".len();
        let Some((_, after_relation)) = Self::read_relation_reference_after_preserving_trailing(
            statement_upper,
            after_references,
        ) else {
            return false;
        };
        let after_relation_text = &statement_upper[after_relation..];
        if after_relation_text.contains('(') {
            return false;
        }
        if after_relation_text.trim_start().is_empty() {
            return !after_relation_text.is_empty();
        }

        if Self::statement_has_any_keyword(
            statement_upper,
            after_relation,
            statement_upper.len(),
            &[
                "MATCH",
                "ON DELETE",
                "ON UPDATE",
                "DEFERRABLE",
                "NOT DEFERRABLE",
                "INITIALLY",
            ],
        ) {
            return false;
        }

        let words = Self::statement_words(after_relation_text);
        !after_relation_text
            .chars()
            .last()
            .is_some_and(|ch| ch.is_whitespace())
            && words.len() <= 2
    }

    pub(super) fn is_reference_rule_context(statement_upper: &str) -> bool {
        let Some(references_position) =
            Self::previous_keyword_position(statement_upper, "REFERENCES")
        else {
            return false;
        };
        let after_references = references_position + "REFERENCES".len();
        let Some((_, after_relation)) = Self::read_relation_reference_after_preserving_trailing(
            statement_upper,
            after_references,
        ) else {
            return false;
        };
        let after_relation_text = &statement_upper[after_relation..];
        if after_relation_text.contains('(') {
            return false;
        }

        let delete_position = Self::previous_keyword_position(statement_upper, "ON DELETE");
        let update_position = Self::previous_keyword_position(statement_upper, "ON UPDATE");
        let Some((action_position, action_keyword)) = [
            delete_position.map(|position| (position, "ON DELETE")),
            update_position.map(|position| (position, "ON UPDATE")),
        ]
        .into_iter()
        .flatten()
        .max_by_key(|(position, _)| *position) else {
            return false;
        };
        if action_position < after_relation {
            return false;
        }

        let after_action = action_position + action_keyword.len();
        if Self::statement_has_any_keyword(
            statement_upper,
            after_action,
            statement_upper.len(),
            &[
                "ON DELETE",
                "ON UPDATE",
                "MATCH",
                "DEFERRABLE",
                "NOT DEFERRABLE",
                "INITIALLY",
            ],
        ) {
            return false;
        }

        let after_action_text = &statement_upper[after_action..];
        if after_action_text.trim_start().is_empty() {
            return !after_action_text.is_empty();
        }

        if after_action_text
            .chars()
            .last()
            .is_some_and(|ch| ch.is_whitespace())
        {
            return false;
        }

        Self::statement_words(after_action_text).len() <= 2
    }

    pub(super) fn is_reference_column_context(statement_upper: &str) -> bool {
        let Some(references_position) =
            Self::previous_keyword_position(statement_upper, "REFERENCES")
        else {
            return false;
        };
        let after_references = references_position + "REFERENCES".len();
        let Some((_, after_relation)) = Self::read_relation_reference_after_preserving_trailing(
            statement_upper,
            after_references,
        ) else {
            return false;
        };

        let open_position = Self::skip_whitespace(statement_upper, after_relation);
        if !statement_upper[open_position..].starts_with('(') {
            return false;
        }

        Self::matching_paren_end(statement_upper, open_position).is_none()
            || statement_upper[open_position..].trim_end().ends_with(',')
    }

    pub fn reference_table_at_position(source: &str, position: Position) -> Option<String> {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = text_upper.get(statement_start..)?.trim_end();
        let statement_source = text_before.get(statement_start..)?;

        let references_position = Self::previous_keyword_position(statement_upper, "REFERENCES")?;
        let after_references = references_position + "REFERENCES".len();
        let (table_name, _) =
            Self::read_relation_reference_after(statement_source, after_references)?;

        let table_name = Self::normalize_relation_reference(&table_name);
        (!table_name.is_empty()).then_some(table_name)
    }
}
