use super::identifier_paths::{
    encode_identifier_parts, last_identifier_separator, split_identifier_path,
};
use super::types::SqlParser;
use crate::placeholder::{SqlPlaceholderDialect, PLACEHOLDER_IDENTIFIER};
use crate::sql_lexical::lexical_rules_for;
use tower_lsp::lsp_types::Position;
use tree_sitter::Node;

impl SqlParser {
    /// Keep member completion while the cursor is inside alias.column.
    /// Decimal points, strings and comments do not introduce member contexts.
    pub(super) fn qualified_member_context(
        source: &str,
        position: Position,
        dialect: SqlPlaceholderDialect,
    ) -> Option<super::types::CompletionContext> {
        let offset = Self::byte_offset_for_position(source, position);
        let before = source.get(..offset)?;
        if before.chars().last().is_some_and(char::is_whitespace) {
            return None;
        }
        let token = Self::identifier_path_token_for_dialect(before, dialect)?;
        let qualifier = Self::column_qualifier_before_position(
            token,
            crate::position::byte_position_at_end(token),
        )?;
        if !qualifier
            .chars()
            .next()
            .is_some_and(|ch| ch.is_alphabetic() || matches!(ch, '_' | '$' | '\"' | '`' | '['))
        {
            return None;
        }
        let token_start = before.len() - token.len();
        let masked = Self::mask_sql_noise(before);
        let dot = Self::mask_sql_noise(token).rfind('.')? + token_start;
        let full_statement = masked.to_ascii_uppercase();
        let start = Self::previous_statement_start(&full_statement, full_statement.len());
        if Self::is_ddl_on_relation_target_context(full_statement[start..].trim_end())
            || Self::is_reference_relation_target_context(full_statement[start..].trim_end())
        {
            return Some(super::types::CompletionContext::FromClause);
        }
        if let Some(context) = Self::analyze_ddl_target_context(full_statement[start..].trim_end())
        {
            return Some(context);
        }
        let statement = masked[..=dot].to_ascii_uppercase();
        Some(if Self::is_dotted_relation_target_context(&statement) {
            super::types::CompletionContext::FromClause
        } else {
            super::types::CompletionContext::TableColumn
        })
    }

    /// 获取表名（用于 TableColumn 上下文）
    /// 如果光标在 table.column 的位置，返回表名
    pub fn get_table_name_for_column(&self, node: Node, source: &str) -> Option<String> {
        let mut current = Some(node);
        while let Some(node) = current {
            if !matches!(
                node.kind(),
                "identifier"
                    | "field"
                    | "object_reference"
                    | "column_name"
                    | "column_reference"
                    | "column_identifier"
                    | "member_expression"
                    | "dotted_name"
            ) {
                break;
            }
            let text = node.utf8_text(source.as_bytes()).ok()?;
            if let Some(qualifier) = Self::identifier_qualifier(text) {
                return Some(qualifier);
            }
            current = node.parent();
        }
        None
    }

    /// Return the table/schema qualifier immediately before the cursor.
    ///
    /// This is more reliable than AST lookup while a user is typing an incomplete
    /// member access such as `alias.` or `alias.col`.
    pub fn column_qualifier_before_position(source: &str, position: Position) -> Option<String> {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset.min(source.len()))?;
        let token = Self::identifier_path_token_before_cursor(text_before)?;
        if token.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
            return None;
        }

        if last_identifier_separator(token) == Some(token.len() - 1) {
            let qualifier_token = token.strip_suffix('.').unwrap_or(token);
            let qualifier = Self::normalize_identifier(qualifier_token);
            return (!qualifier.is_empty()).then_some(qualifier);
        }

        Self::identifier_qualifier(token).map(|qualifier| Self::normalize_identifier(&qualifier))
    }

    pub(crate) fn column_qualifier_at_position(
        &self,
        source: &str,
        position: Position,
    ) -> Option<String> {
        let offset = Self::byte_offset_for_position(source, position);
        let token = self.identifier_path_token(source.get(..offset)?)?;
        Self::column_qualifier_before_position(token, crate::position::byte_position_at_end(token))
    }

    pub(crate) fn identifier_path_token<'a>(&self, text_before: &'a str) -> Option<&'a str> {
        Self::identifier_path_token_for_dialect(text_before, self.placeholder_dialect)
    }

    pub(crate) fn identifier_path_token_for_sql_dialect<'a>(
        text_before: &'a str,
        dialect: &str,
    ) -> Option<&'a str> {
        let rules = lexical_rules_for(dialect);
        let bracket_style = if rules.bracket_identifiers {
            SqlPlaceholderDialect::Generic
        } else {
            SqlPlaceholderDialect::Postgres
        };
        Self::identifier_path_token_for_dialect(text_before, bracket_style)
    }

    /// Normalize an identifier path for cross-dialect matching.
    pub fn normalize_identifier(identifier: &str) -> String {
        encode_identifier_parts(&Self::identifier_parts(identifier))
    }

    /// Return the last segment of a possibly schema-qualified identifier.
    pub fn identifier_last_part(identifier: &str) -> String {
        Self::identifier_parts(identifier)
            .pop()
            .unwrap_or_else(|| identifier.trim().to_string())
    }

    /// Return the qualifier before the last segment of a possibly schema-qualified identifier.
    pub fn identifier_qualifier(identifier: &str) -> Option<String> {
        let mut parts = Self::identifier_parts(identifier);
        if parts.len() < 2 {
            return None;
        }

        parts.pop();
        let qualifier = encode_identifier_parts(&parts);
        if qualifier.is_empty() {
            None
        } else {
            Some(qualifier)
        }
    }

    /// Compare a referenced table name with a schema table.
    pub fn table_name_matches(reference: &str, database: &str, table_name: &str) -> bool {
        Self::table_name_matches_with_catalog(reference, None, database, table_name)
    }

    /// Compare a referenced table name with a catalog-aware schema table.
    /// A server-qualified reference needs an explicit server identity.
    pub fn table_name_matches_with_catalog(
        reference: &str,
        catalog: Option<&str>,
        database: &str,
        table_name: &str,
    ) -> bool {
        Self::table_name_matches_with_namespace(reference, None, catalog, database, table_name)
    }

    pub fn table_name_matches_with_namespace(
        reference: &str,
        server: Option<&str>,
        catalog: Option<&str>,
        database: &str,
        table_name: &str,
    ) -> bool {
        let reference_parts = Self::identifier_parts(reference);
        if reference_parts.len() > 4 || reference_parts.iter().any(String::is_empty) {
            return false;
        }
        if reference_parts.len() == 4
            && !server.is_some_and(|value| Self::identifier_eq(&reference_parts[0], value))
        {
            return false;
        }
        // Schema metadata already contains literal component names.
        let table = table_name;

        match reference_parts.as_slice() {
            [name] => Self::identifier_eq(name, table),
            parts if parts.len() >= 2 => {
                let referenced_table = parts.last().unwrap();
                let referenced_database = &parts[parts.len() - 2];
                if !Self::identifier_eq(referenced_table, table)
                    || (!database.is_empty() && !Self::identifier_eq(referenced_database, database))
                {
                    return false;
                }

                let Some(catalog) = catalog.filter(|catalog| !catalog.is_empty()) else {
                    return true;
                };
                parts.len() < 3 || Self::identifier_eq(&parts[parts.len() - 3], catalog)
            }
            _ => false,
        }
    }

    pub(super) fn normalize_relation_reference(identifier: &str) -> String {
        let parts = Self::identifier_parts(identifier);
        let Some(last) = parts.last() else {
            return String::new();
        };
        if last == PLACEHOLDER_IDENTIFIER {
            return String::new();
        }
        if parts.iter().any(|part| part == PLACEHOLDER_IDENTIFIER) {
            return encode_identifier_parts(std::slice::from_ref(last));
        }
        encode_identifier_parts(&parts)
    }

    pub(crate) fn identifier_parts(identifier: &str) -> Vec<String> {
        split_identifier_path(identifier)
    }

    pub(super) fn identifier_eq(left: &str, right: &str) -> bool {
        left == right || left.eq_ignore_ascii_case(right)
    }

    pub(crate) fn identifier_path_token_before_cursor(text_before: &str) -> Option<&str> {
        Self::identifier_path_token_for_dialect(text_before, SqlPlaceholderDialect::Generic)
    }

    pub(crate) fn identifier_path_token_for_dialect(
        text_before: &str,
        dialect: SqlPlaceholderDialect,
    ) -> Option<&str> {
        let mut token_start = 0;
        let mut quote_end: Option<char> = None;
        let mut chars = text_before.char_indices().peekable();

        while let Some((index, ch)) = chars.next() {
            if let Some(end_quote) = quote_end {
                if ch == end_quote {
                    if chars.peek().is_some_and(|(_, next)| *next == end_quote) {
                        chars.next();
                        continue;
                    }
                    quote_end = None;
                }
                continue;
            }

            if ch == '-' && chars.peek().is_some_and(|(_, next)| *next == '-') {
                chars.next();
                for (at, next) in chars.by_ref() {
                    token_start = at + next.len_utf8();
                    if matches!(next, '\r' | '\n') {
                        break;
                    }
                }
                continue;
            }
            if ch == '/' && chars.peek().is_some_and(|(_, next)| *next == '*') {
                chars.next();
                let mut depth = 1;
                while let Some((at, next)) = chars.next() {
                    token_start = at + next.len_utf8();
                    if next == '/' && chars.peek().is_some_and(|(_, next)| *next == '*') {
                        chars.next();
                        depth += 1;
                    } else if next == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                        chars.next();
                        token_start += 1;
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
                continue;
            }
            match ch {
                '"' | '\'' | '`' => quote_end = Some(ch),
                '[' | ']' if dialect == SqlPlaceholderDialect::Postgres => token_start = index + 1,
                '[' => quote_end = Some(']'),
                _ if Self::is_identifier_token_boundary(ch) => {
                    token_start = index + ch.len_utf8();
                }
                _ => {}
            }
        }

        let token = text_before[token_start..].trim();
        (!token.is_empty()).then_some(token)
    }

    pub(super) fn is_identifier_token_boundary(ch: char) -> bool {
        ch.is_whitespace()
            || matches!(
                ch,
                ',' | '(' | ')' | ';' | '=' | '<' | '>' | '!' | '+' | '-' | '*' | '/' | '%'
            )
    }
}
