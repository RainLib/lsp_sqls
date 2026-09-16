use super::types::{CompletionContext, SqlParser};
use tower_lsp::lsp_types::Position;

impl SqlParser {
    pub(super) fn is_update_set_context(statement_upper: &str) -> bool {
        let Some(update_position) = Self::previous_keyword_position(statement_upper, "UPDATE")
        else {
            return false;
        };
        let after_update = update_position + "UPDATE".len();
        let Some(set_position) = Self::next_keyword_position(statement_upper, "SET", after_update)
        else {
            return false;
        };

        !Self::statement_has_any_keyword(
            statement_upper,
            set_position + "SET".len(),
            statement_upper.len(),
            &["WHERE", "RETURNING"],
        )
    }

    pub(super) fn join_using_column_context_at_position(source: &str, position: Position) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = text_upper[statement_start..].trim_end();

        Self::is_join_using_column_context(statement_upper)
    }

    pub(super) fn order_direction_context_at_position(source: &str, position: Position) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::is_order_direction_context(statement_upper)
    }

    pub(super) fn group_by_continuation_context_at_position(
        source: &str,
        position: Position,
    ) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::is_group_by_continuation_context(statement_upper)
    }

    pub(super) fn case_when_condition_context_at_position(
        source: &str,
        position: Position,
    ) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = text_upper[statement_start..].trim_end();
        let words = Self::statement_words(statement_upper);

        Self::words_end_with(&words, &["WHEN"])
    }

    pub(super) fn simple_case_when_value_context_at_position(
        source: &str,
        position: Position,
    ) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let text_upper = text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::is_simple_case_when_value_context(statement_upper)
    }

    pub(super) fn simple_case_when_value_continuation_context_at_position(
        source: &str,
        position: Position,
    ) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let text_upper = text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::is_simple_case_when_value_continuation_context(statement_upper)
    }

    pub(super) fn case_result_context_at_position(source: &str, position: Position) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let text_upper = text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::is_case_result_context(statement_upper)
    }

    pub(super) fn case_continuation_context_at_position(source: &str, position: Position) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let text_upper = text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::is_case_continuation_context(statement_upper)
    }

    pub(super) fn analyze_relation_continuation_context_at_position(
        source: &str,
        position: Position,
    ) -> Option<CompletionContext> {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::analyze_relation_continuation_context(statement_upper)
    }

    pub(super) fn select_continuation_context_at_position(
        source: &str,
        position: Position,
    ) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let text_upper = text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::is_select_continuation_context(statement_upper)
    }

    pub(super) fn insert_value_context_at_position(source: &str, position: Position) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::is_insert_value_context(statement_upper)
    }

    pub(super) fn insert_continuation_context_at_position(
        source: &str,
        position: Position,
    ) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let raw_text_upper = text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&raw_text_upper, raw_text_upper.len());
        let statement_upper = &raw_text_upper[statement_start..];

        Self::is_insert_continuation_context(statement_upper)
    }

    pub(super) fn insert_conflict_target_context_at_position(
        source: &str,
        position: Position,
    ) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let raw_text_upper = text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&raw_text_upper, raw_text_upper.len());
        let statement_upper = &raw_text_upper[statement_start..];

        Self::is_insert_conflict_target_context(statement_upper)
    }

    pub(super) fn insert_conflict_constraint_context_at_position(
        source: &str,
        position: Position,
    ) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let raw_text_upper = text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&raw_text_upper, raw_text_upper.len());
        let statement_upper = &raw_text_upper[statement_start..];

        Self::is_insert_conflict_constraint_context(statement_upper)
    }

    pub(super) fn insert_conflict_action_context_at_position(
        source: &str,
        position: Position,
    ) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let raw_text_upper = text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&raw_text_upper, raw_text_upper.len());
        let statement_upper = &raw_text_upper[statement_start..];

        Self::is_insert_conflict_action_context(statement_upper)
    }

    pub(super) fn expression_value_context_at_position(source: &str, position: Position) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let raw_text_upper = text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];
        let raw_statement_upper = &raw_text_upper[statement_start..];

        Self::is_expression_value_context(statement_upper, raw_statement_upper)
    }

    pub(super) fn predicate_continuation_context_at_position(
        source: &str,
        position: Position,
    ) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let raw_text_upper = text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];
        let raw_statement_upper = &raw_text_upper[statement_start..];

        Self::is_predicate_continuation_context(statement_upper, raw_statement_upper)
    }

    pub(super) fn reference_column_context_at_position(source: &str, position: Position) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = text_upper[statement_start..].trim_end();

        Self::is_reference_column_context(statement_upper)
    }

    pub(super) fn reference_relation_target_context_at_position(
        source: &str,
        position: Position,
    ) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::is_reference_relation_target_context(statement_upper)
    }

    pub(super) fn reference_action_context_at_position(source: &str, position: Position) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::is_reference_action_context(statement_upper)
    }

    pub(super) fn reference_rule_context_at_position(source: &str, position: Position) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::is_reference_rule_context(statement_upper)
    }
}
