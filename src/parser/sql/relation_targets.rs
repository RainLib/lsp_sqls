use super::types::SqlParser;
use tower_lsp::lsp_types::Position;

impl SqlParser {
    pub(super) fn is_relation_target_context(statement_upper: &str) -> bool {
        let relation_targets = [
            ("INSERT INTO", &["VALUES", "SELECT", "RETURNING"][..]),
            ("UPDATE", &["SET", "WHERE", "RETURNING"][..]),
            ("DELETE FROM", &["WHERE", "RETURNING", "USING"][..]),
            ("TRUNCATE TABLE", &[][..]),
            (
                "ALTER TABLE",
                &[
                    "ADD", "ALTER", "DROP", "RENAME", "OWNER", "SET", "RESET", "VALIDATE",
                    "ENABLE", "DISABLE",
                ][..],
            ),
            ("DROP TABLE", &[][..]),
            ("DROP VIEW", &[][..]),
        ];

        relation_targets.iter().any(|(phrase, terminators)| {
            let Some(position) = Self::previous_keyword_position(statement_upper, phrase) else {
                return false;
            };
            let after_phrase = position + phrase.len();
            !Self::statement_has_any_keyword(
                statement_upper,
                after_phrase,
                statement_upper.len(),
                terminators,
            )
        })
    }

    pub(super) fn is_dotted_relation_target_context(statement_upper: &str) -> bool {
        // FROM inside a CTE or completed sibling subquery must not turn the
        // outer SELECT alias. into a schema/table lookup.
        let (start, _) = Self::innermost_query_scope_start(statement_upper);
        let scoped = Self::mask_nested_parenthesized_regions(&statement_upper[start..]);
        let statement_upper = scoped.trim_end();
        if !statement_upper.ends_with('.') {
            return false;
        }

        let relation_targets = [
            (
                "FROM",
                &[
                    "WHERE",
                    "GROUP BY",
                    "ORDER BY",
                    "HAVING",
                    "LIMIT",
                    "OFFSET",
                    "FETCH",
                    "UNION",
                    "EXCEPT",
                    "INTERSECT",
                    "RETURNING",
                    "JOIN",
                    "INNER JOIN",
                    "LEFT JOIN",
                    "RIGHT JOIN",
                    "FULL JOIN",
                    "CROSS JOIN",
                ][..],
            ),
            (
                "JOIN",
                &["ON", "USING", "WHERE", "GROUP BY", "ORDER BY", "LIMIT"][..],
            ),
            ("INSERT INTO", &["VALUES", "SELECT", "RETURNING"][..]),
            ("UPDATE", &["SET", "WHERE", "RETURNING"][..]),
            ("DELETE FROM", &["WHERE", "RETURNING", "USING"][..]),
            ("TRUNCATE TABLE", &[][..]),
            (
                "ALTER TABLE",
                &[
                    "ADD", "ALTER", "DROP", "RENAME", "OWNER", "SET", "RESET", "VALIDATE",
                    "ENABLE", "DISABLE",
                ][..],
            ),
            ("DROP TABLE", &[][..]),
            ("DROP VIEW", &[][..]),
        ];

        relation_targets.iter().any(|(phrase, terminators)| {
            let Some(position) = Self::previous_keyword_position(statement_upper, phrase) else {
                return false;
            };
            let after_phrase = position + phrase.len();
            !Self::statement_has_any_keyword(
                statement_upper,
                after_phrase,
                statement_upper.len(),
                terminators,
            )
        })
    }

    pub fn ddl_on_relation_target_at_position(source: &str, position: Position) -> bool {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = text_upper[statement_start..].trim_end();

        Self::statement_words(statement_upper)
            .last()
            .is_some_and(|word| *word == "ON")
            && Self::is_ddl_on_relation_target_context(statement_upper)
    }

    pub(super) fn is_ddl_on_relation_target_context(statement_upper: &str) -> bool {
        let Some(on_position) = Self::previous_keyword_position(statement_upper, "ON") else {
            return false;
        };
        if !Self::should_read_on_relation(statement_upper, on_position) {
            return false;
        }

        let after_on = on_position + "ON".len();
        let after_on_text = &statement_upper[after_on..];
        !after_on_text.contains('(')
            && !Self::statement_has_any_keyword(
                statement_upper,
                after_on,
                statement_upper.len(),
                &["WHERE", "EXECUTE", "FOR"],
            )
    }

    pub(super) fn is_ddl_on_column_context(statement_upper: &str) -> bool {
        let Some(on_position) = Self::previous_keyword_position(statement_upper, "ON") else {
            return false;
        };
        if !Self::should_read_on_relation(statement_upper, on_position) {
            return false;
        }

        statement_upper[on_position + "ON".len()..].contains('(')
    }

    pub(super) fn is_insert_column_context(statement_upper: &str) -> bool {
        let Some(into_position) = Self::previous_keyword_position(statement_upper, "INSERT INTO")
        else {
            return false;
        };
        let after_into = into_position + "INSERT INTO".len();
        if Self::statement_has_any_keyword(
            statement_upper,
            after_into,
            statement_upper.len(),
            &["VALUES", "SELECT", "RETURNING"],
        ) {
            return false;
        }

        statement_upper[after_into..].contains('(')
    }
}
