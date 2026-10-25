//! Adapt supported SQL spellings without changing source byte coordinates.
//! Inference checks the original operator/name and actual configured dialect.
use super::apply::adapt_apply_joins;
use super::bracket_identifiers::adapt_bracket_identifiers;
use super::fulltext::adapt_languages;
use super::types::SqlParser;
use crate::placeholder::{normalize_sql_placeholders_for_dialect, SqlPlaceholderDialect};

pub(super) fn normalize_for_parser(sql: &str, dialect: SqlPlaceholderDialect) -> String {
    let normalized = normalize_sql_placeholders_for_dialect(sql, dialect);
    let brackets = dialect == SqlPlaceholderDialect::SqlServer && sql.contains('[');
    let try_cast = sql
        .as_bytes()
        .windows(8)
        .any(|word| matches!(word[0], b'T' | b't') && word.eq_ignore_ascii_case(b"TRY_CAST"));
    let signs = sql.bytes().any(|byte| matches!(byte, b'+' | b'-'));
    let fulltext = sql
        .as_bytes()
        .windows(5)
        .any(|word| matches!(word[0], b'M' | b'm') && word.eq_ignore_ascii_case(b"MATCH"));
    let search_and = matches!(
        dialect,
        SqlPlaceholderDialect::Postgres | SqlPlaceholderDialect::Generic
    ) && sql.contains("&&");
    let mysql_identifiers = matches!(
        dialect,
        SqlPlaceholderDialect::Mysql | SqlPlaceholderDialect::Generic
    ) && sql.contains('`');
    let postgres_identifiers = matches!(
        dialect,
        SqlPlaceholderDialect::Postgres
            | SqlPlaceholderDialect::Generic
            | SqlPlaceholderDialect::SqlServer
    ) && sql.contains('"');
    let language = sql
        .as_bytes()
        .windows(8)
        .any(|word| word.eq_ignore_ascii_case(b"LANGUAGE"));
    let apply = sql
        .as_bytes()
        .windows(5)
        .any(|word| word.eq_ignore_ascii_case(b"APPLY"));
    if !try_cast
        && !signs
        && !fulltext
        && !search_and
        && !mysql_identifiers
        && !postgres_identifiers
        && !language
        && !apply
        && !brackets
    {
        return normalized;
    }
    // Opaque operands retain a marker, so a quote before an infix minus cannot
    // disappear and make that minus look like a SELECT-prefix unary sign.
    let searchable = SqlParser::mask_sql_operand_context(sql, dialect).to_ascii_uppercase();
    let mut bytes = normalized.into_bytes();
    if brackets {
        adapt_bracket_identifiers(sql, &searchable, &mut bytes);
    }
    adapt_try_casts(sql, &searchable, &mut bytes);
    if apply {
        adapt_apply_joins(&searchable, &mut bytes);
    }
    if language {
        adapt_languages(sql, &searchable, &mut bytes);
    }
    if search_and {
        for (index, _) in searchable.match_indices("&&") {
            bytes[index + 1] = b' ';
        }
    }
    if mysql_identifiers || postgres_identifiers {
        // Backticks only accept Latin contents in the shared grammar; doubled
        // double quotes also split incorrectly. Adapt only those spans, keeping
        // ordinary double-quoted text untouched. Consumers read original SQL.
        for (index, byte) in searchable.bytes().enumerate() {
            if (byte == b'x' || byte == b'X')
                && ((mysql_identifiers && sql.as_bytes()[index] == b'`')
                    || (postgres_identifiers && sql.as_bytes()[index] == b'"'))
            {
                if let Some((_, end)) = SqlParser::read_identifier_part(sql, index) {
                    if sql.as_bytes()[index] == b'"' && !sql[index + 1..end - 1].contains("\"\"") {
                        continue;
                    }
                    for byte in &mut bytes[index + 1..end - 1] {
                        if !matches!(*byte, b'\r' | b'\n') {
                            *byte = b'_';
                        }
                    }
                }
            }
        }
    }
    if fulltext {
        for expression in SqlParser::match_against_in_mask(sql, &searchable) {
            expression.adapt(&mut bytes);
        }
    }

    for (index, sign) in searchable.bytes().enumerate() {
        if !matches!(sign, b'+' | b'-') {
            continue;
        }
        let Some(next) = searchable[index + 1..].trim_start().chars().next() else {
            continue;
        };
        if !(next.is_alphabetic() || matches!(next, '_' | '(' | '@' | '+' | '-')) {
            continue;
        }
        let before = searchable[..index].trim_end();
        let boundary = before.chars().next_back().is_none_or(|ch| {
            matches!(
                ch,
                '(' | ',' | '=' | '+' | '-' | '*' | '/' | '%' | '^' | '<' | '>' | '!' | '|' | '&'
            )
        });
        let word = before
            .rsplit(|ch: char| !ch.is_alphanumeric() && ch != '_')
            .next()
            .unwrap_or("");
        let keyword = matches!(
            word,
            "SELECT"
                | "DISTINCT"
                | "THEN"
                | "ELSE"
                | "WHEN"
                | "RETURNING"
                | "BY"
                | "WHERE"
                | "ON"
                | "AND"
                | "OR"
                | "NOT"
                | "HAVING"
                | "VALUES"
                | "SET"
                | "LIMIT"
                | "OFFSET"
        );
        if boundary || keyword {
            bytes[index] = b'~';
        }
    }
    String::from_utf8(bytes).expect("SQL spelling replacements preserve UTF-8")
}

// One scan and a parenthesis stack avoid rescanning every nested TRY_CAST
// argument. Protected regions have already been masked with operand markers.
fn adapt_try_casts(source: &str, sql: &str, output: &mut [u8]) {
    let bytes = sql.as_bytes();
    let mut stack = Vec::<Option<usize>>::new();
    let mut pending = None;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"TRY_CAST") {
            let before = sql[..index].trim_end().chars().next_back();
            let adjacent = source[..index].chars().next_back();
            let after = source[index + 8..].chars().next();
            if !matches!(before, Some('.' | ':'))
                && adjacent.is_none_or(|ch| !SqlParser::is_identifier_char(ch))
                && after.is_none_or(|ch| !SqlParser::is_identifier_char(ch))
            {
                if let Some(offset) = bytes[index + 8..]
                    .iter()
                    .position(|byte| !byte.is_ascii_whitespace())
                {
                    let open = index + 8 + offset;
                    if bytes[open] == b'(' {
                        pending = Some((open, index));
                    }
                }
            }
            index += 8;
            continue;
        }
        match bytes[index] {
            b'(' => stack.push(
                pending
                    .take()
                    .filter(|(open, _)| *open == index)
                    .map(|(_, start)| start),
            ),
            b')' => {
                stack.pop();
            }
            b'A' if bytes.get(index + 1) == Some(&b'S') => {
                let before = sql[..index].chars().next_back();
                let after = sql[index + 2..].chars().next();
                if before.is_none_or(|ch| !SqlParser::is_identifier_char(ch))
                    && after.is_none_or(|ch| !SqlParser::is_identifier_char(ch))
                {
                    if let Some(Some(start)) = stack.last() {
                        output[*start..*start + 8].copy_from_slice(b"CAST    ");
                    }
                }
            }
            _ => {}
        }
        index += 1;
    }
}

impl SqlParser {
    pub(crate) fn source_node_text<'s>(node: tree_sitter::Node<'_>, source: &'s str) -> &'s str {
        if node.kind() == "keyword_cast" {
            if let Some((start, end)) = Self::try_cast_name_range(node, source) {
                return &source[start..end];
            }
        }
        if let Some((start, end)) = Self::apply_keyword_range(node, source) {
            return &source[start..end];
        }
        if let Some((start, end)) = Self::search_operator_range(node, source) {
            return &source[start..end];
        }
        node.utf8_text(source.as_bytes()).unwrap_or("")
    }

    pub(super) fn search_operator_range(
        node: tree_sitter::Node<'_>,
        source: &str,
    ) -> Option<(usize, usize)> {
        let start = node.start_byte();
        (node.kind() == "op_other" && source.get(start..start + 2) == Some("&&"))
            .then_some((start, start + 2))
    }

    pub(crate) fn try_cast_name_range(
        node: tree_sitter::Node<'_>,
        source: &str,
    ) -> Option<(usize, usize)> {
        if !matches!(node.kind(), "cast" | "keyword_cast") {
            return None;
        }
        let start = node.start_byte();
        let end = start.checked_add(8)?;
        source
            .get(start..end)
            .filter(|name| name.eq_ignore_ascii_case("TRY_CAST"))
            .map(|_| (start, end))
    }
}
