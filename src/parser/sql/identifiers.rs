use super::types::SqlParser;
use crate::placeholder::PLACEHOLDER_IDENTIFIER;
use crate::token::Keywords;
use tower_lsp::lsp_types::Position;
use tree_sitter::Node;

impl SqlParser {
    /// 获取表名（用于 TableColumn 上下文）
    /// 如果光标在 table.column 的位置，返回表名
    pub fn get_table_name_for_column(&self, node: Node, source: &str) -> Option<String> {
        let mut current = Some(node);

        while let Some(n) = current {
            let kind = n.kind();

            // Try to extract from text directly first, if it looks like "table." or "table.col"
            if let Ok(text) = n.utf8_text(source.as_bytes()) {
                if let Some(dot_pos) = text.rfind('.') {
                    let table_name = text[..dot_pos].trim();
                    if !table_name.is_empty() && !Keywords::is_keyword(table_name) {
                        return Some(Self::normalize_identifier(table_name));
                    }
                }
            }

            // 查找 member_expression 或 dotted_name
            if kind == "member_expression" || kind == "dotted_name" {
                if let Ok(text) = n.utf8_text(source.as_bytes()) {
                    if let Some(dot_pos) = text.rfind('.') {
                        let table_name = text[..dot_pos].trim();
                        if !table_name.is_empty() && !Keywords::is_keyword(table_name) {
                            return Some(Self::normalize_identifier(table_name));
                        }
                    }
                }
            }

            // 检查父节点
            if let Some(parent) = n.parent() {
                if let Ok(text) = parent.utf8_text(source.as_bytes()) {
                    if let Some(dot_pos) = text.rfind('.') {
                        let table_name = text[..dot_pos].trim();
                        if !table_name.is_empty() && !Keywords::is_keyword(table_name) {
                            return Some(Self::normalize_identifier(table_name));
                        }
                    }
                }
            }

            current = n.parent();
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

        if token.ends_with('.') {
            let qualifier_token = token.strip_suffix('.').unwrap_or(token);
            let qualifier_token = if qualifier_token.ends_with('.') {
                format!("{qualifier_token}dbo")
            } else {
                qualifier_token.to_string()
            };
            let qualifier = Self::normalize_identifier(&qualifier_token);
            return (!qualifier.is_empty()).then_some(qualifier);
        }

        Self::identifier_qualifier(token).map(|qualifier| Self::normalize_identifier(&qualifier))
    }

    /// Normalize an identifier path for cross-dialect matching.
    pub fn normalize_identifier(identifier: &str) -> String {
        Self::identifier_parts(identifier).join(".")
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
        let qualifier = parts.join(".");
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
    /// Leading server segments are allowed, while the catalog/schema suffix
    /// must match when the SQL reference supplies it.
    pub fn table_name_matches_with_catalog(
        reference: &str,
        catalog: Option<&str>,
        database: &str,
        table_name: &str,
    ) -> bool {
        let reference_parts = Self::identifier_parts(reference);
        let table = Self::normalize_identifier(table_name);
        let database = Self::normalize_identifier(database);

        match reference_parts.as_slice() {
            [name] => Self::identifier_eq(name, &table),
            parts if parts.len() >= 2 => {
                let referenced_table = parts.last().unwrap();
                let referenced_database = &parts[parts.len() - 2];
                if !Self::identifier_eq(referenced_table, &table)
                    || (!database.is_empty()
                        && !Self::identifier_eq(referenced_database, &database))
                {
                    return false;
                }

                let Some(catalog) = catalog
                    .map(Self::normalize_identifier)
                    .filter(|catalog| !catalog.is_empty())
                else {
                    return true;
                };
                parts.len() < 3 || Self::identifier_eq(&parts[parts.len() - 3], &catalog)
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
            return last.clone();
        }
        parts.join(".")
    }

    pub(super) fn identifier_parts(identifier: &str) -> Vec<String> {
        let identifier = identifier
            .trim()
            .trim_matches(|ch: char| matches!(ch, ',' | ';' | '(' | ')'));
        let raw_parts = identifier.split('.').collect::<Vec<_>>();
        raw_parts
            .iter()
            .enumerate()
            .filter_map(|(index, part)| {
                let normalized = Self::unquote_identifier_part(part);
                if !normalized.is_empty() {
                    return Some(normalized);
                }
                (index > 0 && index + 1 < raw_parts.len()).then(|| "dbo".to_string())
            })
            .collect()
    }

    pub(super) fn unquote_identifier_part(part: &str) -> String {
        let trimmed = part
            .trim()
            .trim_matches(|ch: char| matches!(ch, ',' | ';' | '(' | ')'));

        if trimmed.len() >= 2 {
            let bytes = trimmed.as_bytes();
            let first = bytes[0] as char;
            let last = bytes[bytes.len() - 1] as char;

            if (first == '"' && last == '"')
                || (first == '`' && last == '`')
                || (first == '\'' && last == '\'')
                || (first == '[' && last == ']')
            {
                return trimmed[1..trimmed.len() - 1]
                    .replace("\"\"", "\"")
                    .replace("``", "`")
                    .replace("''", "'");
            }
        }

        trimmed.to_string()
    }

    pub(super) fn identifier_eq(left: &str, right: &str) -> bool {
        left == right || left.eq_ignore_ascii_case(right)
    }

    pub(super) fn identifier_path_token_before_cursor(text_before: &str) -> Option<&str> {
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

            match ch {
                '"' | '\'' | '`' => quote_end = Some(ch),
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
