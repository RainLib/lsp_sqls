use super::types::SqlParser;

impl SqlParser {
    pub(super) fn latest_predicate_clause(statement_upper: &str) -> Option<(usize, &'static str)> {
        [
            (
                "WHERE",
                Self::previous_keyword_position(statement_upper, "WHERE"),
            ),
            (
                "HAVING",
                Self::previous_keyword_position(statement_upper, "HAVING"),
            ),
            ("ON", Self::previous_keyword_position(statement_upper, "ON")),
            (
                "WHEN",
                Self::previous_keyword_position(statement_upper, "WHEN"),
            ),
            (
                "SET",
                Self::previous_keyword_position(statement_upper, "SET"),
            ),
            (
                "UPDATE",
                Self::previous_keyword_position(statement_upper, "UPDATE"),
            ),
        ]
        .into_iter()
        .filter_map(|(clause, position)| position.map(|position| (position, clause)))
        .max_by_key(|(position, _)| *position)
    }

    pub(super) fn predicate_clause_has_later_terminator(
        statement_upper: &str,
        after_clause: usize,
        clause: &str,
    ) -> bool {
        let terminators = match clause {
            "SET" => &["WHERE", "RETURNING"][..],
            "WHERE" | "ON" => &[
                "GROUP BY",
                "ORDER BY",
                "HAVING",
                "LIMIT",
                "RETURNING",
                "UNION",
            ][..],
            "HAVING" => &["ORDER BY", "LIMIT", "UNION"][..],
            "WHEN" => &["THEN", "ELSE", "END"][..],
            _ => &[][..],
        };

        Self::statement_has_any_keyword(
            statement_upper,
            after_clause,
            statement_upper.len(),
            terminators,
        )
    }

    pub(super) fn is_expression_value_context(
        statement_upper: &str,
        raw_statement_upper: &str,
    ) -> bool {
        let Some((clause_position, clause)) = Self::latest_predicate_clause(statement_upper) else {
            return false;
        };

        let after_clause = clause_position + clause.len();
        if Self::predicate_clause_has_later_terminator(statement_upper, after_clause, clause) {
            return false;
        }

        let segment = &statement_upper[after_clause..];
        let raw_segment = raw_statement_upper.get(after_clause..).unwrap_or(segment);
        let trimmed = segment.trim_end();
        if trimmed.is_empty() || trimmed.ends_with('.') {
            return false;
        }

        let Some(operator_end) = Self::latest_value_operator_end(trimmed) else {
            return false;
        };
        let raw_after_operator = raw_segment.get(operator_end..).unwrap_or("");
        let value_text = raw_after_operator.trim_start();
        let value_text_trimmed = value_text.trim_end();
        if value_text_trimmed.is_empty() {
            return true;
        }

        if raw_after_operator
            .chars()
            .last()
            .is_some_and(|ch| ch.is_whitespace())
        {
            if value_text_trimmed.ends_with(',')
                && Self::is_in_list_value_operator(trimmed, operator_end)
            {
                return true;
            }
            return false;
        }

        let value_text_upper = value_text_trimmed.to_ascii_uppercase();
        let words = Self::statement_words(&value_text_upper);
        let Some(prefix) = words.last().copied() else {
            return false;
        };
        if !Self::is_value_keyword_prefix(prefix) {
            return false;
        }

        let Some(prefix_start) = value_text_upper.rfind(prefix) else {
            return false;
        };
        let before_prefix = value_text_trimmed[..prefix_start].trim_end();
        before_prefix.trim().is_empty()
            || (before_prefix.ends_with(',')
                && Self::is_in_list_value_operator(trimmed, operator_end))
    }

    pub(super) fn latest_value_operator_end(segment_upper: &str) -> Option<usize> {
        let trimmed_len = segment_upper.trim_end().len();
        let raw_segment = &segment_upper[..trimmed_len];
        let searchable_segment = Self::mask_nested_parenthesized_regions(raw_segment);
        let segment = searchable_segment.as_str();
        let mut best: Option<(usize, usize)> = None;

        let mut consider = |position: usize, end: usize| {
            if best.is_none_or(|(best_position, best_end)| {
                position > best_position || (position == best_position && end > best_end)
            }) {
                best = Some((position, end));
            }
        };

        [
            "!=", "<>", "<=", ">=", "=", "<", ">", "+", "-", "*", "/", "%",
        ]
        .iter()
        .filter_map(|operator| {
            segment
                .rfind(operator)
                .map(|position| (position, position + operator.len()))
        })
        .for_each(|(position, end)| consider(position, end));

        for operator in [
            "LIKE", "ILIKE", "RLIKE", "REGEXP", "IS NOT", "IS", "NOT IN", "IN", "BETWEEN",
        ] {
            if let Some(position) = Self::previous_keyword_position(segment, operator) {
                let mut end = position + operator.len();
                if matches!(operator, "IN" | "NOT IN") {
                    end = Self::extend_operator_end_through_open_paren(segment, end);
                }
                consider(position, end);
            }
        }

        if let Some(between_position) = Self::previous_keyword_position(segment, "BETWEEN") {
            let after_between = between_position + "BETWEEN".len();
            if let Some(and_relative) =
                Self::previous_keyword_position(&segment[after_between..], "AND")
            {
                let and_position = after_between + and_relative;
                consider(and_position, and_position + "AND".len());
            }
        }

        best.map(|(_, end)| end)
    }

    pub(super) fn extend_operator_end_through_open_paren(
        segment_upper: &str,
        operator_end: usize,
    ) -> usize {
        let Some(after_operator) = segment_upper.get(operator_end..) else {
            return operator_end;
        };
        let leading_whitespace = after_operator
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
        if after_operator[leading_whitespace..].starts_with('(') {
            operator_end + leading_whitespace + 1
        } else {
            operator_end
        }
    }

    pub(super) fn is_in_list_value_operator(segment_upper: &str, operator_end: usize) -> bool {
        let trimmed_len = segment_upper.trim_end().len();
        let raw_segment = &segment_upper[..trimmed_len];
        let searchable_segment = Self::mask_nested_parenthesized_regions(raw_segment);
        let Some(before_operator_end) = searchable_segment.get(..operator_end) else {
            return false;
        };
        let before_operator_end = before_operator_end.trim_end();
        if !before_operator_end.ends_with('(') {
            return false;
        }

        let before_open = before_operator_end[..before_operator_end.len() - 1].trim_end();
        let words = Self::statement_words(before_open);
        matches!(words.as_slice(), [.., "IN"])
    }

    pub(super) fn is_predicate_continuation_context(
        statement_upper: &str,
        raw_statement_upper: &str,
    ) -> bool {
        let Some((clause_position, clause)) = Self::latest_predicate_clause(statement_upper) else {
            return false;
        };

        let after_clause = clause_position + clause.len();
        if Self::predicate_clause_has_later_terminator(statement_upper, after_clause, clause) {
            return false;
        }

        let segment = &statement_upper[after_clause..];
        let raw_segment = raw_statement_upper.get(after_clause..).unwrap_or(segment);
        let trimmed = segment.trim_end();
        if trimmed.is_empty() || trimmed.ends_with('.') {
            return false;
        }

        let Some(operator_end) = Self::latest_value_operator_end(trimmed) else {
            return false;
        };
        let raw_after_operator = raw_segment.get(operator_end..).unwrap_or("");
        let Some(value_segment) =
            Self::predicate_continuation_value_segment(raw_after_operator, clause)
        else {
            return false;
        };

        Self::is_completed_value_expression(value_segment)
    }

    pub(super) fn predicate_continuation_value_segment<'a>(
        raw_after_operator: &'a str,
        clause: &str,
    ) -> Option<&'a str> {
        let trimmed_end = raw_after_operator.trim_end();
        if trimmed_end.trim().is_empty() {
            return None;
        }

        if raw_after_operator
            .chars()
            .last()
            .is_some_and(|ch| ch.is_whitespace())
        {
            let value_segment = trimmed_end.trim();
            return (!value_segment.ends_with(',')).then_some(value_segment);
        }

        let upper = trimmed_end.to_ascii_uppercase();
        let words = Self::statement_words(&upper);
        let prefix = words.last().copied()?;
        if !Self::is_predicate_continuation_prefix(prefix, clause) {
            return None;
        }

        let prefix_start = upper.rfind(prefix)?;
        let before_prefix = &trimmed_end[..prefix_start];
        if !before_prefix
            .chars()
            .last()
            .is_some_and(|ch| ch.is_whitespace())
        {
            return None;
        }

        let value_segment = before_prefix.trim();
        (!value_segment.ends_with(',')).then_some(value_segment)
    }

    pub(super) fn is_completed_value_expression(value_segment: &str) -> bool {
        let value = value_segment.trim();
        if value.is_empty() || value.ends_with(',') {
            return false;
        }
        if value.ends_with(')') {
            return true;
        }
        if value.ends_with('\'') || value.ends_with('"') {
            return true;
        }

        let token = value
            .split(|ch: char| ch.is_whitespace() || ch == ',')
            .rfind(|token| !token.is_empty())
            .unwrap_or("")
            .trim_matches(|ch| matches!(ch, '\'' | '"' | '`' | '[' | ']' | ';'));
        if token.is_empty() {
            return false;
        }

        let token_upper = token.to_ascii_uppercase();
        matches!(
            token_upper.as_str(),
            "DEFAULT"
                | "NULL"
                | "TRUE"
                | "FALSE"
                | "CURRENT_DATE"
                | "CURRENT_TIMESTAMP"
                | "NOW"
                | "NOW()"
                | "TODAY"
                | "TODAY()"
        ) || token.parse::<f64>().is_ok()
            || token.starts_with('$')
            || token.starts_with(':')
            || token == "?"
    }

    pub(super) fn is_predicate_continuation_prefix(prefix: &str, clause: &str) -> bool {
        matches!(
            prefix,
            "A" | "AN" | "AND" | "O" | "OR" | "L" | "LI" | "LIM" | "LIMI" | "LIMIT"
        ) || matches!(
            (clause, prefix),
            (
                "WHERE" | "ON",
                "G" | "GR"
                    | "GRO"
                    | "GROU"
                    | "GROUP"
                    | "H"
                    | "HA"
                    | "HAV"
                    | "HAVI"
                    | "HAVIN"
                    | "HAVING"
                    | "ORD"
                    | "ORDE"
                    | "ORDER"
            ) | ("HAVING", "ORD" | "ORDE" | "ORDER")
                | ("WHEN", "T" | "TH" | "THE" | "THEN")
                | ("ON" | "SET", "W" | "WH" | "WHE" | "WHER" | "WHERE")
                | (
                    "SET",
                    "R" | "RE"
                        | "RET"
                        | "RETU"
                        | "RETUR"
                        | "RETURN"
                        | "RETURNI"
                        | "RETURNIN"
                        | "RETURNING"
                )
        )
    }
}
