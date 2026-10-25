use super::types::SqlParser;
use crate::placeholder::placeholder_at;
use crate::placeholder::PLACEHOLDER_IDENTIFIER;
use crate::token::Keywords;

impl SqlParser {
    pub(super) fn extract_on_relation_references(
        source: &str,
        source_upper: &str,
        tables: &mut Vec<String>,
    ) {
        let mut search_pos = 0;
        while let Some(on_position) = Self::next_keyword_position(source_upper, "ON", search_pos) {
            let after_on = on_position + "ON".len();
            if Self::should_read_on_relation(source_upper, on_position) {
                if let Some((table_name, _)) = Self::read_relation_reference_after(source, after_on)
                {
                    Self::push_table_reference(tables, &table_name);
                }
            }
            search_pos = after_on;
        }
    }

    pub(super) fn skip_whitespace(source: &str, mut index: usize) -> usize {
        while index < source.len() {
            let Some(ch) = source[index..].chars().next() else {
                break;
            };
            if !ch.is_whitespace() {
                break;
            }
            index += ch.len_utf8();
        }
        index
    }

    pub(super) fn consume_word(source: &str, index: usize, word: &str) -> Option<usize> {
        let index = Self::skip_whitespace(source, index);
        let end = index + word.len();
        if !source
            .get(index..end)
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(word))
        {
            return None;
        }

        let after_is_boundary = if end >= source.len() {
            true
        } else {
            source[end..]
                .chars()
                .next()
                .is_none_or(|ch| !Self::is_identifier_char(ch))
        };

        after_is_boundary.then_some(end)
    }

    pub(super) fn skip_relation_modifiers(source: &str, mut index: usize) -> usize {
        if let Some(next_index) = Self::consume_word(source, index, "ONLY") {
            index = next_index;
        }

        // LATERAL is contextual: SQL Server can have a real table named
        // `lateral`, while PostgreSQL uses it as a modifier only before a
        // subquery or table function.
        if let Some(after_lateral) = Self::consume_word(source, index, "LATERAL") {
            let candidate = Self::skip_whitespace(source, after_lateral);
            let is_lateral_source = source[candidate..].starts_with('(')
                || Self::read_identifier_path(source, candidate).is_some_and(
                    |(_, after_reference)| {
                        let after_reference = Self::skip_whitespace(source, after_reference);
                        source[after_reference..].starts_with('(')
                    },
                );
            if is_lateral_source {
                index = candidate;
            }
        }

        let after_if = Self::consume_word(source, index, "IF");
        if let Some(after_if) = after_if {
            let after_not = Self::consume_word(source, after_if, "NOT").unwrap_or(after_if);
            if let Some(after_exists) = Self::consume_word(source, after_not, "EXISTS") {
                index = after_exists;
            }
        }

        Self::skip_whitespace(source, index)
    }

    pub(crate) fn read_identifier_part(source: &str, index: usize) -> Option<(String, usize)> {
        let index = Self::skip_whitespace(source, index);
        if let Some(span) = placeholder_at(source, index) {
            return Some((PLACEHOLDER_IDENTIFIER.to_string(), span.end));
        }
        let first = source[index..].chars().next()?;

        if matches!(first, '"' | '`' | '\'') {
            let quote = first;
            let mut end = index + quote.len_utf8();
            let mut part = String::new();
            while end < source.len() {
                let ch = source[end..].chars().next()?;
                end += ch.len_utf8();
                if ch == quote {
                    if source[end..].starts_with(quote) {
                        part.push(quote);
                        end += quote.len_utf8();
                        continue;
                    }
                    return Some((part, end));
                }
                part.push(ch);
            }
            return None;
        }

        if first == '[' {
            let mut end = index + 1;
            let mut part = String::new();
            while end < source.len() {
                let ch = source[end..].chars().next()?;
                end += ch.len_utf8();
                if ch == ']' {
                    if source[end..].starts_with(']') {
                        part.push(']');
                        end += 1;
                        continue;
                    }
                    return Some((part, end));
                }
                part.push(ch);
            }
            return None;
        }

        if !Self::is_identifier_char(first) {
            return None;
        }

        let mut end = index;
        while end < source.len() {
            let Some(ch) = source[end..].chars().next() else {
                break;
            };
            if !Self::is_identifier_char(ch) {
                break;
            }
            end += ch.len_utf8();
        }

        Some((source[index..end].to_string(), end))
    }

    pub(super) fn read_identifier_path(source: &str, index: usize) -> Option<(String, usize)> {
        let mut parts = Vec::new();
        let (first_part, mut index) = Self::read_identifier_part(source, index)?;
        parts.push(first_part);

        loop {
            index = Self::skip_whitespace(source, index);
            if !source[index..].starts_with('.') {
                break;
            }
            index += 1;
            let next_part = Self::skip_whitespace(source, index);
            if source[next_part..].starts_with('.') {
                parts.push(String::new());
                index = next_part;
                continue;
            }
            let Some((part, next_index)) = Self::read_identifier_part(source, index) else {
                break;
            };
            parts.push(part);
            index = next_index;
        }

        Some((Self::identifier_path_from_parts(&parts), index))
    }

    pub(super) fn skip_parenthesized_region(source: &str, index: usize) -> usize {
        let mut index = Self::skip_whitespace(source, index);
        if !source[index..].starts_with('(') {
            return index;
        }

        let mut depth = 0usize;
        while index < source.len() {
            let Some(ch) = source[index..].chars().next() else {
                break;
            };
            if ch == '(' {
                depth += 1;
            } else if ch == ')' {
                depth = depth.saturating_sub(1);
                index += ch.len_utf8();
                if depth == 0 {
                    break;
                }
                continue;
            }
            index += ch.len_utf8();
        }

        index
    }

    pub(super) fn read_relation_reference_after(
        source: &str,
        index: usize,
    ) -> Option<(String, usize)> {
        let index = Self::skip_relation_modifiers(source, index);
        if let Some(reference) = Self::read_table_variable_reference(source, index) {
            return Some(reference);
        }
        Self::read_identifier_path(source, index)
    }

    // In a row-source position, a T-SQL @name is an identifier rather than
    // the opaque named placeholder used by scalar-expression scanning.
    fn read_table_variable_reference(source: &str, index: usize) -> Option<(String, usize)> {
        if !source[index..].starts_with('@') || source[index..].starts_with("@@") {
            return None;
        }
        let end = crate::tsql_identifier::prefix_end(source, index)?;
        crate::tsql_identifier::is_complete_name(&source[index..end])
            .then(|| (source[index..end].to_string(), end))
    }

    pub(super) fn read_relation_reference_after_preserving_trailing(
        source: &str,
        index: usize,
    ) -> Option<(String, usize)> {
        let index = Self::skip_relation_modifiers(source, index);
        if let Some(reference) = Self::read_table_variable_reference(source, index) {
            return Some(reference);
        }
        Self::read_identifier_path_preserving_trailing(source, index)
    }

    pub(super) fn read_identifier_path_preserving_trailing(
        source: &str,
        index: usize,
    ) -> Option<(String, usize)> {
        let mut parts = Vec::new();
        let (first_part, mut index) = Self::read_identifier_part(source, index)?;
        parts.push(first_part);

        loop {
            let after_part = index;
            let dot_index = Self::skip_whitespace(source, index);
            if !source[dot_index..].starts_with('.') {
                return Some((Self::identifier_path_from_parts(&parts), after_part));
            }
            index = dot_index + 1;
            let next_part = Self::skip_whitespace(source, index);
            if source[next_part..].starts_with('.') {
                parts.push(String::new());
                index = next_part;
                continue;
            }
            let Some((part, next_index)) = Self::read_identifier_part(source, index) else {
                return Some((Self::identifier_path_from_parts(&parts), after_part));
            };
            parts.push(part);
            index = next_index;
        }
    }

    pub(super) fn read_relation_alias_with_span_after(
        source: &str,
        searchable_source: &str,
        index: usize,
    ) -> Option<(String, usize, usize)> {
        // Quoted identifiers are intentionally masked in `searchable_source`.
        // Whitespace navigation must therefore use the original SQL or it can
        // skip across a quoted alias and land on the following clause.
        let mut index = Self::skip_whitespace(source, index);
        // Table functions place their argument list between the relation name
        // and alias. PostgreSQL may additionally insert WITH ORDINALITY.
        if searchable_source[index..].starts_with('(') {
            index = Self::skip_parenthesized_region(searchable_source, index);
            if let Some(after_with) = Self::consume_word(searchable_source, index, "WITH") {
                if let Some(after_ordinality) =
                    Self::consume_word(searchable_source, after_with, "ORDINALITY")
                {
                    index = after_ordinality;
                } else {
                    // OPENJSON's WITH schema is after the argument list. This
                    // branch cannot consume WITH table hints on a base table.
                    let declaration = Self::skip_whitespace(source, after_with);
                    if searchable_source[declaration..].starts_with('(') {
                        index = Self::skip_parenthesized_region(searchable_source, declaration);
                    }
                }
            }
        }
        if let Some(after_as) = Self::consume_word(searchable_source, index, "AS") {
            index = after_as;
        }

        let alias_start = Self::skip_whitespace(source, index);
        let (alias, next_index) = Self::read_identifier_part(source, alias_start)?;
        let quoted = matches!(source.as_bytes().get(alias_start), Some(b'"' | b'`' | b'['));
        if !quoted
            && (Keywords::is_keyword(&alias)
                || matches!(
                    alias.to_ascii_uppercase().as_str(),
                    "ON" | "USING" | "WHERE" | "SET" | "VALUES" | "RETURNING" | "GROUP" | "ORDER"
                ))
        {
            return None;
        }

        Some((alias, alias_start, next_index))
    }

    pub(super) fn comma_separated_relation_starts(source_upper: &str) -> Vec<usize> {
        const FROM_BOUNDARIES: &[&str] = &[
            "WHERE",
            "GROUP",
            "HAVING",
            "ORDER",
            "LIMIT",
            "OFFSET",
            "FETCH",
            "WINDOW",
            "QUALIFY",
            "RETURNING",
            "UNION",
            "EXCEPT",
            "INTERSECT",
        ];

        let mut starts = Vec::new();
        let mut from_search = 0;
        while let Some(from_position) =
            Self::next_keyword_position(source_upper, "FROM", from_search)
        {
            let mut index = from_position + "FROM".len();
            let mut depth = 0usize;
            while index < source_upper.len() {
                let Some(ch) = source_upper[index..].chars().next() else {
                    break;
                };
                if depth == 0 {
                    if matches!(ch, ';' | ')')
                        || FROM_BOUNDARIES.iter().any(|keyword| {
                            source_upper[index..].starts_with(keyword)
                                && Self::is_keyword_at(source_upper, index, keyword)
                        })
                    {
                        break;
                    }
                    if ch == ',' {
                        starts.push(index + ch.len_utf8());
                    }
                }
                match ch {
                    '(' => depth += 1,
                    ')' if depth > 0 => depth -= 1,
                    _ => {}
                }
                index += ch.len_utf8();
            }
            from_search = (from_position + "FROM".len()).max(index);
        }
        starts
    }
}
