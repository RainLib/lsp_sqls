use super::types::{CompletionContext, SqlParser};

impl SqlParser {
    pub(super) fn analyze_relation_continuation_context(
        statement_upper: &str,
    ) -> Option<CompletionContext> {
        if Self::is_join_condition_context(statement_upper) {
            return Some(CompletionContext::JoinConditionClause);
        }

        if Self::is_from_continuation_context(statement_upper) {
            return Some(CompletionContext::FromContinuationClause);
        }

        None
    }

    pub(super) fn is_join_condition_context(statement_upper: &str) -> bool {
        let Some(join_position) = Self::previous_keyword_position(statement_upper, "JOIN") else {
            return false;
        };
        let after_join = join_position + "JOIN".len();
        if Self::statement_has_any_keyword(
            statement_upper,
            after_join,
            statement_upper.len(),
            &[
                "ON", "USING", "WHERE", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", "UNION",
            ],
        ) {
            return false;
        }

        Self::relation_target_completed_or_prefixed(
            &statement_upper[after_join..],
            &["A", "AS", "O", "ON", "U", "US", "USI", "USIN", "USING"],
        )
    }

    pub(super) fn is_from_continuation_context(statement_upper: &str) -> bool {
        let Some(from_position) = Self::previous_keyword_position(statement_upper, "FROM") else {
            return false;
        };
        let after_from = from_position + "FROM".len();
        if Self::statement_has_any_keyword(
            statement_upper,
            after_from,
            statement_upper.len(),
            &[
                "JOIN", "WHERE", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", "OFFSET", "FETCH",
                "UNION", "ON", "USING",
            ],
        ) {
            return false;
        }

        let segment = statement_upper[after_from..]
            .rsplit(',')
            .next()
            .unwrap_or(&statement_upper[after_from..]);
        Self::relation_target_completed_or_prefixed(
            segment,
            &[
                "A", "AS", "J", "JO", "JOI", "JOIN", "I", "IN", "INN", "INNE", "INNER", "L", "LE",
                "LEF", "LEFT", "R", "RI", "RIG", "RIGH", "RIGHT", "F", "FU", "FUL", "FULL", "C",
                "CR", "CRO", "CROS", "CROSS", "W", "WH", "WHE", "WHER", "WHERE", "G", "GR", "GRO",
                "GROU", "GROUP", "O", "OR", "ORD", "ORDE", "ORDER", "H", "HA", "HAV", "HAVI", "Q",
                "QU", "QUA", "QUAL", "QUALI", "QUALIF", "QUALIFY", "HAVIN", "HAVING", "LI", "LIM",
                "LIMI", "LIMIT", "OF", "OFF", "OFFS", "OFFSE", "OFFSET", "FE", "FET", "FETC",
                "FETCH", "LA", "LAT", "LATE", "LATER", "LATERA", "LATERAL",
            ],
        )
    }

    pub(super) fn relation_target_completed_or_prefixed(
        segment: &str,
        continuation_prefixes: &[&str],
    ) -> bool {
        let trimmed = segment.trim_start();
        let trimmed_end = trimmed.trim_end();
        if trimmed_end.is_empty() || trimmed_end.ends_with('.') || trimmed_end.ends_with('(') {
            return false;
        }

        let words = Self::statement_words(trimmed_end);
        if words.is_empty() {
            return false;
        }

        if trimmed.chars().last().is_some_and(|ch| ch.is_whitespace()) {
            return words.last().is_some_and(|word| *word != "AS");
        }

        if words.len() < 2 {
            return false;
        }

        let Some(prefix) = words.last().copied() else {
            return false;
        };
        continuation_prefixes.contains(&prefix)
    }

    pub(super) fn is_select_continuation_context(statement_upper: &str) -> bool {
        let Some(select_position) = Self::previous_keyword_position(statement_upper, "SELECT")
        else {
            return false;
        };
        let after_select = select_position + "SELECT".len();
        if Self::statement_has_any_keyword(
            statement_upper,
            after_select,
            statement_upper.len(),
            &[
                "FROM", "WHERE", "GROUP BY", "ORDER BY", "HAVING", "QUALIFY", "LIMIT", "UNION",
            ],
        ) {
            return false;
        }

        let segment = statement_upper[after_select..]
            .rsplit(',')
            .next()
            .unwrap_or(&statement_upper[after_select..]);
        Self::select_item_completed_or_prefixed(
            segment,
            &[
                "A", "AS", "F", "FR", "FRO", "FROM", "Q", "QU", "QUA", "QUAL", "QUALI", "QUALIF",
                "QUALIFY",
            ],
        )
    }

    pub(super) fn select_item_completed_or_prefixed(
        segment: &str,
        continuation_prefixes: &[&str],
    ) -> bool {
        let trimmed = segment.trim_start();
        let trimmed_end = trimmed.trim_end();
        if trimmed_end.is_empty() || trimmed_end.ends_with('.') || trimmed_end.ends_with('(') {
            return false;
        }

        let words = Self::statement_words(trimmed_end);
        let meaningful_words = words
            .iter()
            .filter(|word| **word != "DISTINCT")
            .copied()
            .collect::<Vec<_>>();
        let has_wildcard = trimmed_end.contains('*');

        if trimmed.chars().last().is_some_and(|ch| ch.is_whitespace()) {
            if words.last().is_some_and(|word| *word == "AS") {
                return false;
            }
            return has_wildcard || !meaningful_words.is_empty();
        }

        let Some(prefix) = words.last().copied() else {
            return false;
        };
        if !continuation_prefixes.contains(&prefix) {
            return false;
        }

        if words.len() >= 2 {
            return true;
        }

        let Some((before_prefix, _)) = trimmed.rsplit_once(prefix) else {
            return false;
        };
        has_wildcard
            || Self::statement_words(before_prefix)
                .iter()
                .any(|word| *word != "DISTINCT")
    }
}
