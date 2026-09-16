use super::types::{CompletionContext, SqlParser};
use tower_lsp::lsp_types::Position;

impl SqlParser {
    pub(super) fn analyze_ddl_target_context_at_position(
        source: &str,
        position: Position,
    ) -> Option<CompletionContext> {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = text_upper[statement_start..].trim_end();

        Self::analyze_ddl_target_context(statement_upper)
    }

    pub(super) fn alter_table_action_context_at_position(source: &str, position: Position) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::is_alter_table_action_context(statement_upper)
    }

    pub(super) fn analyze_dml_action_context_at_position(
        source: &str,
        position: Position,
    ) -> Option<CompletionContext> {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::analyze_dml_action_context(statement_upper)
    }

    pub(super) fn data_type_context_at_position(source: &str, position: Position) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];

        Self::is_data_type_context(statement_upper)
    }

    pub(super) fn analyze_dml_action_context(statement_upper: &str) -> Option<CompletionContext> {
        if Self::is_relation_action_context(statement_upper, "INSERT INTO") {
            return Some(CompletionContext::InsertActionClause);
        }
        if !Self::is_insert_scoped_update_context(statement_upper)
            && Self::is_relation_action_context(statement_upper, "UPDATE")
        {
            return Some(CompletionContext::UpdateActionClause);
        }
        if Self::is_relation_action_context(statement_upper, "DELETE FROM") {
            return Some(CompletionContext::DeleteActionClause);
        }

        None
    }

    pub(super) fn is_insert_scoped_update_context(statement_upper: &str) -> bool {
        let Some(update_position) = Self::previous_keyword_position(statement_upper, "UPDATE")
        else {
            return false;
        };
        let Some(insert_position) = Self::previous_keyword_position(statement_upper, "INSERT INTO")
        else {
            return false;
        };
        if insert_position > update_position {
            return false;
        }

        let before_update = &statement_upper[..update_position];
        Self::previous_keyword_position(before_update, "ON CONFLICT").is_some()
            || Self::previous_keyword_position(before_update, "ON DUPLICATE KEY").is_some()
    }

    pub(super) fn is_data_type_context(statement_upper: &str) -> bool {
        Self::is_create_table_column_type_context(statement_upper)
            || Self::is_alter_table_add_column_type_context(statement_upper)
            || Self::is_alter_table_modify_column_type_context(statement_upper)
            || Self::is_alter_table_change_column_type_context(statement_upper)
            || Self::is_alter_table_alter_column_type_context(statement_upper)
    }

    pub(super) fn is_create_table_column_type_context(statement_upper: &str) -> bool {
        let Some(create_table_position) =
            Self::previous_keyword_position(statement_upper, "CREATE TABLE")
        else {
            return false;
        };
        let Some(open_position) = statement_upper[create_table_position..]
            .find('(')
            .map(|position| create_table_position + position)
        else {
            return false;
        };

        if Self::matching_paren_end(statement_upper, open_position).is_some() {
            return false;
        }

        let segment_start = statement_upper[open_position + 1..]
            .rfind(',')
            .map(|position| open_position + 1 + position + 1)
            .unwrap_or(open_position + 1);
        Self::is_column_definition_type_segment(&statement_upper[segment_start..])
    }

    pub(super) fn is_alter_table_add_column_type_context(statement_upper: &str) -> bool {
        if Self::is_alter_table_column_definition_type_after_phrase(statement_upper, "ADD COLUMN") {
            return true;
        }

        Self::is_alter_table_column_definition_type_after_phrase(statement_upper, "ADD")
    }

    pub(super) fn is_alter_table_modify_column_type_context(statement_upper: &str) -> bool {
        if Self::is_alter_table_column_definition_type_after_phrase(
            statement_upper,
            "MODIFY COLUMN",
        ) {
            return true;
        }

        Self::is_alter_table_column_definition_type_after_phrase(statement_upper, "MODIFY")
    }

    pub(super) fn is_alter_table_column_definition_type_after_phrase(
        statement_upper: &str,
        phrase: &str,
    ) -> bool {
        let Some(alter_table_position) =
            Self::previous_keyword_position(statement_upper, "ALTER TABLE")
        else {
            return false;
        };
        let Some(phrase_position) = Self::previous_keyword_position(statement_upper, phrase) else {
            return false;
        };
        if phrase_position < alter_table_position {
            return false;
        }

        let after_phrase = phrase_position + phrase.len();
        if Self::statement_has_any_keyword(
            statement_upper,
            after_phrase,
            statement_upper.len(),
            &[
                "NOT NULL",
                "NULL",
                "DEFAULT",
                "PRIMARY KEY",
                "UNIQUE",
                "CHECK",
                "REFERENCES",
                "CONSTRAINT",
            ],
        ) {
            return false;
        }

        Self::is_column_definition_type_segment(&statement_upper[after_phrase..])
    }

    pub(super) fn is_alter_table_change_column_type_context(statement_upper: &str) -> bool {
        if Self::is_alter_table_change_column_type_after_phrase(statement_upper, "CHANGE COLUMN") {
            return true;
        }

        Self::is_alter_table_change_column_type_after_phrase(statement_upper, "CHANGE")
    }

    pub(super) fn is_alter_table_change_column_type_after_phrase(
        statement_upper: &str,
        phrase: &str,
    ) -> bool {
        let Some(alter_table_position) =
            Self::previous_keyword_position(statement_upper, "ALTER TABLE")
        else {
            return false;
        };
        let Some(phrase_position) = Self::previous_keyword_position(statement_upper, phrase) else {
            return false;
        };
        if phrase_position < alter_table_position {
            return false;
        }

        let after_phrase = phrase_position + phrase.len();
        if Self::statement_has_any_keyword(
            statement_upper,
            after_phrase,
            statement_upper.len(),
            &[
                "NOT NULL",
                "NULL",
                "DEFAULT",
                "PRIMARY KEY",
                "UNIQUE",
                "CHECK",
                "REFERENCES",
                "CONSTRAINT",
            ],
        ) {
            return false;
        }

        Self::is_column_change_type_segment(&statement_upper[after_phrase..])
    }

    pub(super) fn is_alter_table_alter_column_type_context(statement_upper: &str) -> bool {
        let Some(alter_table_position) =
            Self::previous_keyword_position(statement_upper, "ALTER TABLE")
        else {
            return false;
        };
        let Some(alter_column_position) =
            Self::previous_keyword_position(statement_upper, "ALTER COLUMN")
        else {
            return false;
        };
        if alter_column_position < alter_table_position {
            return false;
        }

        let Some(type_position) = Self::previous_keyword_position(statement_upper, "TYPE") else {
            return false;
        };
        if type_position < alter_column_position {
            return false;
        }

        let after_type = type_position + "TYPE".len();
        if Self::statement_has_any_keyword(
            statement_upper,
            after_type,
            statement_upper.len(),
            &[
                "USING",
                "COLLATE",
                "NOT NULL",
                "NULL",
                "DEFAULT",
                "PRIMARY KEY",
                "UNIQUE",
                "CHECK",
                "REFERENCES",
            ],
        ) {
            return false;
        }

        Self::is_data_type_name_segment(&statement_upper[after_type..])
    }

    pub(super) fn is_column_definition_type_segment(segment: &str) -> bool {
        let trimmed = segment.trim_start();
        if trimmed.is_empty() {
            return false;
        }

        let first_word = Self::statement_words(trimmed)
            .first()
            .copied()
            .unwrap_or("");
        if matches!(
            first_word,
            "PRIMARY"
                | "FOREIGN"
                | "UNIQUE"
                | "CHECK"
                | "CONSTRAINT"
                | "COLUMN"
                | "KEY"
                | "INDEX"
                | "FULLTEXT"
                | "SPATIAL"
                | "EXCLUDE"
                | "LIKE"
        ) {
            return false;
        }

        let has_separator_after_column_name = trimmed
            .split_once(|ch: char| ch.is_whitespace())
            .is_some_and(|(_, rest)| {
                !rest.is_empty() || segment.chars().last().is_some_and(|ch| ch.is_whitespace())
            });
        if !has_separator_after_column_name {
            return false;
        }

        let words = Self::statement_words(trimmed);
        if trimmed.chars().last().is_some_and(|ch| ch.is_whitespace()) {
            return words.len() == 1;
        }

        words.len() == 2
    }

    pub(super) fn is_column_change_type_segment(segment: &str) -> bool {
        let trimmed = segment.trim_start();
        if trimmed.is_empty() {
            return false;
        }

        let words = Self::statement_words(trimmed);
        if trimmed.chars().last().is_some_and(|ch| ch.is_whitespace()) {
            return words.len() == 2;
        }

        words.len() == 3
    }

    pub(super) fn is_data_type_name_segment(segment: &str) -> bool {
        let trimmed = segment.trim_start();
        if trimmed.is_empty() {
            return !segment.is_empty();
        }

        if trimmed.chars().last().is_some_and(|ch| ch.is_whitespace()) {
            return false;
        }

        Self::statement_words(trimmed).len() == 1
    }

    pub(super) fn analyze_ddl_target_context(statement_upper: &str) -> Option<CompletionContext> {
        if Self::is_alter_table_target_context(
            statement_upper,
            &[
                "DROP COLUMN",
                "ALTER COLUMN",
                "RENAME COLUMN",
                "MODIFY COLUMN",
                "MODIFY",
                "CHANGE COLUMN",
                "CHANGE",
            ],
            &[
                "TYPE",
                "SET",
                "DROP",
                "RESTART",
                "TO",
                "CASCADE",
                "RESTRICT",
                "ADD",
                "ALTER",
                "RENAME",
                "NOT NULL",
                "NULL",
                "DEFAULT",
                "PRIMARY KEY",
                "UNIQUE",
                "CHECK",
                "REFERENCES",
            ],
        ) {
            return Some(CompletionContext::ColumnTargetClause);
        }

        if Self::is_alter_table_target_context(
            statement_upper,
            &["DROP CONSTRAINT", "RENAME CONSTRAINT"],
            &[
                "TO", "CASCADE", "RESTRICT", "ADD", "ALTER", "DROP", "RENAME",
            ],
        ) {
            return Some(CompletionContext::ConstraintTargetClause);
        }

        if Self::is_global_index_target_context(statement_upper)
            || Self::is_alter_table_target_context(
                statement_upper,
                &["DROP INDEX", "RENAME INDEX"],
                &["TO", "ADD", "ALTER", "DROP", "RENAME"],
            )
        {
            return Some(CompletionContext::IndexTargetClause);
        }

        None
    }

    pub(super) fn is_global_index_target_context(statement_upper: &str) -> bool {
        ["DROP INDEX", "ALTER INDEX", "REINDEX INDEX"]
            .iter()
            .any(|phrase| {
                let Some(position) = Self::previous_keyword_position(statement_upper, phrase)
                else {
                    return false;
                };
                let after_phrase = position + phrase.len();
                if Self::statement_has_any_keyword(
                    statement_upper,
                    after_phrase,
                    statement_upper.len(),
                    &["ON", "TO", "SET", "RESET", "ALTER", "CASCADE", "RESTRICT"],
                ) {
                    return false;
                }

                let target_text = statement_upper[after_phrase..].trim_start();
                if target_text.is_empty() {
                    return true;
                }
                if target_text
                    .chars()
                    .last()
                    .is_some_and(|ch| ch.is_whitespace())
                {
                    return false;
                }

                Self::statement_words(target_text).len() <= 1
            })
    }

    pub(super) fn is_alter_table_action_context(statement_upper: &str) -> bool {
        let Some(alter_table_position) =
            Self::previous_keyword_position(statement_upper, "ALTER TABLE")
        else {
            return false;
        };
        let after_alter_table = alter_table_position + "ALTER TABLE".len();
        let Some((_, after_table)) = Self::read_relation_reference_after_preserving_trailing(
            statement_upper,
            after_alter_table,
        ) else {
            return false;
        };

        let trailing = &statement_upper[after_table..];
        let trimmed = trailing.trim_start();
        if trimmed.is_empty() {
            return !trailing.is_empty();
        }
        if trimmed.chars().last().is_some_and(|ch| ch.is_whitespace()) {
            return false;
        }

        Self::statement_words(trimmed).len() == 1
    }

    pub(super) fn is_relation_action_context(statement_upper: &str, phrase: &str) -> bool {
        let Some(phrase_position) = Self::previous_keyword_position(statement_upper, phrase) else {
            return false;
        };
        let after_phrase = phrase_position + phrase.len();
        let Some((_, after_table)) =
            Self::read_relation_reference_after_preserving_trailing(statement_upper, after_phrase)
        else {
            return false;
        };

        let trailing = &statement_upper[after_table..];
        let trimmed = trailing.trim_start();
        if trimmed.is_empty() {
            return !trailing.is_empty();
        }
        if trimmed.chars().last().is_some_and(|ch| ch.is_whitespace()) {
            return false;
        }

        Self::statement_words(trimmed).len() == 1
    }

    pub(super) fn is_alter_table_target_context(
        statement_upper: &str,
        target_phrases: &[&str],
        terminators: &[&str],
    ) -> bool {
        let Some(alter_table_position) =
            Self::previous_keyword_position(statement_upper, "ALTER TABLE")
        else {
            return false;
        };

        target_phrases.iter().any(|phrase| {
            let Some(target_position) = Self::previous_keyword_position(statement_upper, phrase)
            else {
                return false;
            };
            if target_position < alter_table_position {
                return false;
            }

            let after_phrase = target_position + phrase.len();
            if Self::statement_has_any_keyword(
                statement_upper,
                after_phrase,
                statement_upper.len(),
                terminators,
            ) {
                return false;
            }

            let target_text = statement_upper[after_phrase..].trim_start();
            if target_text.is_empty() {
                return true;
            }
            if target_text
                .chars()
                .last()
                .is_some_and(|ch| ch.is_whitespace())
            {
                return false;
            }

            Self::statement_words(target_text).len() <= 1
        })
    }
}
