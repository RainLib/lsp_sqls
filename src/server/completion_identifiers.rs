//! Completion identifiers for the SQL language server.
use super::completion_context::position_to_byte_offset;
use super::preferences::KeywordCase;
use crate::dialects::common::identifier_match_rank;
use crate::parser::SqlParser;
use tower_lsp::lsp_types::*;
pub(super) fn add_referenced_alias_completions(
    text: &str,
    position: Position,
    items: &mut Vec<CompletionItem>,
) {
    if relation_alias_context_at_position(text, position) {
        return;
    }
    let offset = position_to_byte_offset(text, position);
    let Some(text_before) = text.get(..offset) else {
        return;
    };
    let Some(prefix_start) = identifier_path_start_before_cursor(text_before) else {
        return;
    };
    let prefix_sql = text_before[prefix_start..].trim();
    if prefix_sql.is_empty() || prefix_sql.contains('.') {
        return;
    }
    let masked = SqlParser::mask_sql_noise(text_before);
    if masked
        .get(prefix_start..)
        .is_none_or(|prefix| prefix.trim().is_empty())
    {
        return;
    }
    let prefix = prefix_sql
        .trim_start_matches(['"', '`', '['])
        .trim_end_matches(['"', '`', ']'])
        .to_lowercase();
    if prefix.is_empty() {
        return;
    }

    // Alias discovery is a bounded, text-only scope scan.  It deliberately
    // avoids constructing a second tree-sitter parse after the dialect has
    // already produced its semantic completion candidates.
    let aliases = SqlParser::relation_aliases_at_position(text, position);
    for alias in aliases {
        let Some(match_rank) = identifier_match_rank(&alias.name, &prefix) else {
            continue;
        };
        let normalized = alias.name.to_lowercase();
        if items.iter().any(|item| {
            item.kind == Some(CompletionItemKind::VARIABLE)
                && item.label == alias.name
                && item.insert_text.as_deref() == Some(alias.sql.as_str())
        }) {
            continue;
        }
        items.push(CompletionItem {
            label: alias.name.clone(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail: Some(format!("Table alias · {}", alias.relation)),
            filter_text: Some(alias.name.clone()),
            insert_text: Some(alias.sql),
            sort_text: Some(format!("-2:alias:{match_rank}:{normalized}")),
            ..Default::default()
        });
    }
}

pub(super) fn keyword_with_completion_case(keyword: &str, keyword_case: KeywordCase) -> String {
    match keyword_case {
        KeywordCase::Upper | KeywordCase::Preserve => keyword.to_ascii_uppercase(),
        KeywordCase::Lower => keyword.to_ascii_lowercase(),
    }
}

pub(super) const SQL_COMPLETION_IDENTIFIER_KEYWORDS: &[&str] = &[
    "all",
    "alter",
    "and",
    "as",
    "by",
    "case",
    "check",
    "constraint",
    "create",
    "default",
    "delete",
    "distinct",
    "drop",
    "else",
    "end",
    "except",
    "false",
    "foreign",
    "from",
    "group",
    "having",
    "index",
    "insert",
    "intersect",
    "into",
    "join",
    "key",
    "limit",
    "not",
    "null",
    "offset",
    "on",
    "or",
    "order",
    "primary",
    "recursive",
    "references",
    "returning",
    "select",
    "set",
    "table",
    "then",
    "true",
    "union",
    "unique",
    "update",
    "using",
    "values",
    "view",
    "when",
    "where",
    "with",
];

pub(super) fn quote_completion_identifier(identifier: &str, dialect_name: &str) -> String {
    if ((identifier.starts_with('"') && identifier.ends_with('"'))
        || (identifier.starts_with('`') && identifier.ends_with('`'))
        || (identifier.starts_with('[') && identifier.ends_with(']')))
        && identifier.len() >= 2
    {
        return identifier.to_string();
    }
    let simple_lower = identifier
        .chars()
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_lowercase())
        && identifier.chars().skip(1).all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '$')
        });
    if simple_lower
        && !SQL_COMPLETION_IDENTIFIER_KEYWORDS
            .iter()
            .any(|keyword| identifier.eq_ignore_ascii_case(keyword))
    {
        return identifier.to_string();
    }

    if matches!(dialect_name, "mysql" | "hive" | "clickhouse") {
        format!("`{}`", identifier.replace('`', "``"))
    } else if dialect_name == "sqlserver" {
        format!("[{}]", identifier.replace(']', "]]"))
    } else {
        format!("\"{}\"", identifier.replace('"', "\"\""))
    }
}

pub(super) fn update_completion_text_edit(
    item: &mut CompletionItem,
    transform: &impl Fn(&str) -> String,
) {
    match item.text_edit.as_mut() {
        Some(CompletionTextEdit::Edit(edit)) => edit.new_text = transform(&edit.new_text),
        Some(CompletionTextEdit::InsertAndReplace(edit)) => {
            edit.new_text = transform(&edit.new_text)
        }
        None => {}
    }
}

pub(super) fn relation_alias_context_at_position(text: &str, position: Position) -> bool {
    let offset = position_to_byte_offset(text, position);
    let Some(text_before) = text.get(..offset) else {
        return false;
    };
    let mut last_clause = None;
    for token in text_before
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter(|token| !token.is_empty())
    {
        let token = token.to_ascii_uppercase();
        if matches!(
            token.as_str(),
            "SELECT"
                | "FROM"
                | "JOIN"
                | "WHERE"
                | "ON"
                | "GROUP"
                | "HAVING"
                | "ORDER"
                | "LIMIT"
                | "UNION"
                | "VALUES"
                | "SET"
                | "INTO"
        ) {
            last_clause = Some(token);
        }
    }
    matches!(last_clause.as_deref(), Some("FROM" | "JOIN"))
}

pub(super) fn table_alias_initials(label: &str) -> Option<String> {
    let relation = label.rsplit('.').next()?.trim_matches(['`', '"', '[', ']']);
    let mut alias = String::new();
    let mut take_next = true;
    let mut previous_lowercase = false;
    for character in relation.chars() {
        if !character.is_ascii_alphanumeric() {
            take_next = true;
            previous_lowercase = false;
            continue;
        }
        if take_next || (previous_lowercase && character.is_ascii_uppercase()) {
            alias.push(character.to_ascii_lowercase());
            if alias.len() == 4 {
                break;
            }
        }
        take_next = false;
        previous_lowercase = character.is_ascii_lowercase();
    }
    (!alias.is_empty()).then_some(alias)
}

pub(super) fn qualified_identifier_range_at_position(
    text: &str,
    position: Position,
) -> Option<Range> {
    let offset = position_to_byte_offset(text, position);
    let text_before = text.get(..offset)?;
    let token_start = identifier_path_start_before_cursor(text_before)?;
    let token = text_before[token_start..].trim();

    token.contains('.').then_some(Range {
        start: byte_offset_to_lsp_position(text, token_start),
        end: position,
    })
}

pub(super) fn identifier_path_start_before_cursor(text_before: &str) -> Option<usize> {
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
            _ if is_identifier_path_boundary(ch) => token_start = index + ch.len_utf8(),
            _ => {}
        }
    }

    (token_start < text_before.len()).then_some(token_start)
}

pub(super) fn is_identifier_path_boundary(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            ',' | '(' | ')' | ';' | '=' | '<' | '>' | '!' | '+' | '-' | '*' | '/' | '%'
        )
}

pub(super) fn byte_offset_to_lsp_position(text: &str, target_offset: usize) -> Position {
    let target_offset = target_offset.min(text.len());
    let mut line = 0u32;
    let mut character = 0u32;

    for (byte_index, ch) in text.char_indices() {
        if byte_index >= target_offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16() as u32;
        }
    }

    Position { line, character }
}

pub(super) fn apply_qualified_identifier_completion_edits(
    text: &str,
    position: Position,
    items: &mut [CompletionItem],
) {
    let Some(range) = qualified_identifier_range_at_position(text, position) else {
        return;
    };

    for item in items {
        if item.text_edit.is_some() {
            continue;
        }

        let insert_text = item
            .insert_text
            .clone()
            .unwrap_or_else(|| item.label.clone());
        if !insert_text.contains('.') {
            continue;
        }

        item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: insert_text,
        }));
    }
}
