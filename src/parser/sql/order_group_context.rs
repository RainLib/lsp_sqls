use super::types::SqlParser;

impl SqlParser {
    pub(super) fn is_join_using_column_context(statement_upper: &str) -> bool {
        let Some(using_position) = Self::previous_keyword_position(statement_upper, "USING") else {
            return false;
        };
        let Some(join_position) =
            Self::previous_keyword_position(&statement_upper[..using_position], "JOIN")
        else {
            return false;
        };
        if join_position > using_position {
            return false;
        }

        let after_using = using_position + "USING".len();
        if Self::statement_has_any_keyword(
            statement_upper,
            after_using,
            statement_upper.len(),
            &["WHERE", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", "UNION"],
        ) {
            return false;
        }

        let text_after_using = &statement_upper[after_using..];
        let Some(open_position) = text_after_using.find('(') else {
            return text_after_using.trim().is_empty();
        };

        let text_after_open = &text_after_using[open_position..];
        let mut depth = 0isize;
        for ch in text_after_open.chars() {
            if ch == '(' {
                depth += 1;
            } else if ch == ')' {
                depth -= 1;
            }
        }

        depth > 0 || text_after_open.trim_end().ends_with(',')
    }

    pub(super) fn is_order_direction_context(statement_upper: &str) -> bool {
        let Some(order_position) = Self::previous_keyword_position(statement_upper, "ORDER BY")
        else {
            return false;
        };
        let after_order = order_position + "ORDER BY".len();
        if Self::statement_has_any_keyword(
            statement_upper,
            after_order,
            statement_upper.len(),
            &["LIMIT", "OFFSET", "FETCH", "UNION", "HAVING"],
        ) {
            return false;
        }

        let segment_start = statement_upper[after_order..]
            .rfind(',')
            .map(|position| after_order + position + 1)
            .unwrap_or(after_order);
        let segment = &statement_upper[segment_start..];
        let trimmed = segment.trim_start();
        if trimmed.is_empty() || trimmed.ends_with('.') || trimmed.ends_with('(') {
            return false;
        }

        let words = Self::statement_words(trimmed);
        if words.is_empty() {
            return false;
        }

        if trimmed.chars().last().is_some_and(|ch| ch.is_whitespace()) {
            return true;
        }

        if words.len() < 2 {
            return false;
        }

        let Some(prefix) = words.last().copied() else {
            return false;
        };
        let is_order_modifier_prefix = matches!(
            prefix,
            "A" | "AS"
                | "ASC"
                | "D"
                | "DE"
                | "DES"
                | "DESC"
                | "N"
                | "NU"
                | "NUL"
                | "NULL"
                | "NULLS"
                | "F"
                | "FI"
                | "FIR"
                | "FIRS"
                | "FIRST"
                | "FE"
                | "FET"
                | "FETC"
                | "FETCH"
                | "L"
                | "LI"
                | "LIM"
                | "LIMI"
                | "LIMIT"
                | "LA"
                | "LAS"
                | "LAST"
                | "O"
                | "OF"
                | "OFF"
                | "OFFS"
                | "OFFSE"
                | "OFFSET"
                | "W"
                | "WI"
                | "WIT"
                | "WITH"
        );
        if is_order_modifier_prefix {
            return true;
        }

        match words.as_slice() {
            [_expression, prefix] => matches!(
                *prefix,
                "A" | "AS"
                    | "ASC"
                    | "D"
                    | "DE"
                    | "DES"
                    | "DESC"
                    | "N"
                    | "NU"
                    | "NUL"
                    | "NULL"
                    | "NULLS"
            ),
            [_expression, direction, prefix] if matches!(*direction, "ASC" | "DESC") => {
                matches!(*prefix, "N" | "NU" | "NUL" | "NULL" | "NULLS")
                    || matches!(
                        *prefix,
                        "F" | "FI" | "FIR" | "FIRS" | "FIRST" | "L" | "LA" | "LAS" | "LAST"
                    )
            }
            [_expression, "NULLS", prefix] => matches!(
                *prefix,
                "F" | "FI" | "FIR" | "FIRS" | "FIRST" | "L" | "LA" | "LAS" | "LAST"
            ),
            _ => false,
        }
    }

    pub(super) fn is_group_by_continuation_context(statement_upper: &str) -> bool {
        let Some(group_position) = Self::previous_keyword_position(statement_upper, "GROUP BY")
        else {
            return false;
        };
        let after_group = group_position + "GROUP BY".len();
        if Self::statement_has_any_keyword(
            statement_upper,
            after_group,
            statement_upper.len(),
            &[
                "HAVING", "ORDER BY", "LIMIT", "OFFSET", "FETCH", "UNION", "WHERE",
            ],
        ) {
            return false;
        }

        let segment_start = statement_upper[after_group..]
            .rfind(',')
            .map(|position| after_group + position + 1)
            .unwrap_or(after_group);
        let segment = &statement_upper[segment_start..];
        let trimmed = segment.trim_start();
        if trimmed.is_empty() || trimmed.ends_with('.') || trimmed.ends_with('(') {
            return false;
        }

        let words = Self::statement_words(trimmed);
        if words.is_empty() {
            return false;
        }

        if trimmed.chars().last().is_some_and(|ch| ch.is_whitespace()) {
            return true;
        }

        if words.len() < 2 {
            return false;
        }

        let Some(prefix) = words.last().copied() else {
            return false;
        };
        matches!(
            prefix,
            "H" | "HA"
                | "HAV"
                | "HAVI"
                | "HAVIN"
                | "HAVING"
                | "O"
                | "OR"
                | "ORD"
                | "ORDE"
                | "ORDER"
                | "L"
                | "LI"
                | "LIM"
                | "LIMI"
                | "LIMIT"
                | "OF"
                | "OFF"
                | "OFFS"
                | "OFFSE"
                | "OFFSET"
                | "F"
                | "FE"
                | "FET"
                | "FETC"
                | "FETCH"
                | "W"
                | "WI"
                | "WIT"
                | "WITH"
                | "S"
                | "SO"
                | "SOR"
                | "SORT"
                | "C"
                | "CL"
                | "CLU"
                | "CLUS"
                | "CLUST"
                | "CLUSTE"
                | "CLUSTER"
                | "D"
                | "DI"
                | "DIS"
                | "DIST"
                | "DISTR"
                | "DISTRI"
                | "DISTRIB"
                | "DISTRIBU"
                | "DISTRIBUT"
                | "DISTRIBUTE"
        )
    }
}
