//! Completion context for the SQL language server.
use crate::parser::SqlParser;
use crate::position::lsp_position_at_end;
use std::collections::HashSet;
use tower_lsp::lsp_types::*;
pub(super) fn position_to_byte_offset(text: &str, position: Position) -> usize {
    let (line_start, line_end) = line_bounds_for_position(text, position.line);
    line_start + utf16_position_to_line_byte_offset(&text[line_start..line_end], position.character)
}

/// Recover a high-confidence active statement for completion when users keep
/// multiple query blocks in one console but omit a semicolon between them.
///
/// This is intentionally narrower than the execution splitter. A blank line,
/// a top-level statement keyword, and balanced parentheses are required. CTE,
/// INSERT SELECT, EXPLAIN, CREATE body, ClickHouse mutation, and set-operation
/// continuations remain a single scope.
pub(super) fn completion_statement_prefix(
    text: &str,
    position: Position,
) -> Option<(&str, Position)> {
    let cursor = position_to_byte_offset(text, position);
    let prefix = text.get(..cursor)?;
    let masked = SqlParser::mask_sql_noise(prefix);
    let semicolon_start = masked.rfind(';').map(|offset| offset + 1).unwrap_or(0);
    let start = SqlParser::active_statement_start(prefix);
    if start == semicolon_start {
        return None;
    }
    let scoped = text.get(start..cursor)?;
    Some((scoped, lsp_position_at_end(scoped)))
}

pub(super) fn line_bounds_for_position(text: &str, target_line: u32) -> (usize, usize) {
    let bytes = text.as_bytes();
    let mut current_line = 0u32;
    let mut line_start = 0usize;
    let mut index = 0usize;

    while index < bytes.len() && current_line < target_line {
        match bytes[index] {
            b'\n' => {
                current_line += 1;
                index += 1;
                line_start = index;
            }
            b'\r' => {
                current_line += 1;
                index += 1;
                if index < bytes.len() && bytes[index] == b'\n' {
                    index += 1;
                }
                line_start = index;
            }
            _ => index += 1,
        }
    }

    if current_line < target_line {
        return (text.len(), text.len());
    }

    let mut line_end = line_start;
    while line_end < bytes.len() && !matches!(bytes[line_end], b'\n' | b'\r') {
        line_end += 1;
    }

    (line_start, line_end)
}

pub(super) fn utf16_position_to_line_byte_offset(line: &str, character: u32) -> usize {
    let target_units = character as usize;
    let mut current_units = 0usize;

    for (byte_index, ch) in line.char_indices() {
        if current_units >= target_units {
            return byte_index;
        }

        let next_units = current_units + ch.len_utf16();
        if next_units > target_units {
            return byte_index;
        }

        current_units = next_units;
    }

    line.len()
}

pub(super) const COMPLETED_SQL_CONTEXT_KEYWORDS: &[&str] = &[
    "select", "from", "join", "where", "on", "by", "having", "limit", "offset", "values", "set",
    "into",
];

// Mirrors DBX's static frequency boost. The alphabetical rank deliberately
// stays behind the semantic group and ahead of the label: Monaco's local
// numeric usage rank can still be inserted between the group and this rank.
pub(super) const HIGH_FREQUENCY_SQL_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "AND", "OR", "JOIN", "ON", "IN", "AS", "GROUP BY", "ORDER BY",
    "LEFT", "RIGHT", "INNER", "OUTER", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE",
    "NOT", "NULL", "IS", "LIKE", "DISTINCT", "HAVING", "LIMIT", "COUNT", "SUM", "AVG", "MAX",
    "MIN", "CASE", "UNION", "ALL", "ASC", "DESC", "BETWEEN", "EXISTS",
];

pub(super) fn apply_high_frequency_keyword_ranking(items: &mut [CompletionItem]) {
    for item in items {
        if item.kind != Some(CompletionItemKind::KEYWORD) {
            continue;
        }

        let frequency_rank = if HIGH_FREQUENCY_SQL_KEYWORDS
            .iter()
            .any(|keyword| item.label.eq_ignore_ascii_case(keyword))
        {
            "a"
        } else {
            "b"
        };
        let fallback_label = item.label.to_ascii_lowercase();
        let current = item.sort_text.as_deref().unwrap_or_default();

        if let Some((semantic_group, remainder)) = current.split_once(':') {
            item.sort_text = Some(format!(
                "{semantic_group}:{frequency_rank}:{}",
                remainder.to_ascii_lowercase()
            ));
            continue;
        }

        let bytes = current.as_bytes();
        let mut group_end = usize::from(bytes.first() == Some(&b'-'));
        while bytes
            .get(group_end)
            .is_some_and(|character| character.is_ascii_digit())
        {
            group_end += 1;
        }
        let has_numeric_group = group_end > 0 && !(group_end == 1 && bytes.first() == Some(&b'-'));
        if has_numeric_group {
            let semantic_group = &current[..group_end];
            let remainder = current[group_end..].trim_start_matches(':');
            let normalized_remainder = if remainder.is_empty() {
                fallback_label.clone()
            } else {
                remainder.to_ascii_lowercase()
            };
            item.sort_text = Some(format!(
                "{semantic_group}:{frequency_rank}:{normalized_remainder}"
            ));
        } else {
            item.sort_text = Some(format!("0:{frequency_rank}:{fallback_label}"));
        }
    }
}

pub(super) fn completed_sql_context_keyword_at_position(
    text: &str,
    position: Position,
) -> Option<String> {
    let offset = position_to_byte_offset(text, position);
    let text_before = text.get(..offset)?;

    if text_before
        .chars()
        .last()
        .is_none_or(|ch| ch.is_whitespace())
    {
        return None;
    }

    let token_start = text_before
        .char_indices()
        .rev()
        .find_map(|(index, ch)| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                None
            } else {
                Some(index + ch.len_utf8())
            }
        })
        .unwrap_or(0);
    let token = text_before[token_start..].to_ascii_lowercase();

    COMPLETED_SQL_CONTEXT_KEYWORDS
        .contains(&token.as_str())
        .then_some(token)
}

pub(super) fn apply_completed_sql_context_completion_edits(
    text: &str,
    position: Position,
    items: &mut Vec<CompletionItem>,
) {
    let Some(keyword) = completed_sql_context_keyword_at_position(text, position) else {
        return;
    };
    let byte_position = SqlParser::lsp_position_to_byte_position(text, position);
    let ddl_on_relation_target =
        keyword == "on" && SqlParser::ddl_on_relation_target_at_position(text, byte_position);
    items.retain(|item| {
        completion_item_allowed_after_completed_keyword(&keyword, item, ddl_on_relation_target)
    });

    let range = Range {
        start: position,
        end: position,
    };

    for item in items {
        let insert_text = item
            .insert_text
            .clone()
            .unwrap_or_else(|| item.label.clone());
        item.filter_text = Some(keyword.clone());
        item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: format!(" {insert_text}"),
        }));
    }
}

pub(super) fn completion_item_allowed_after_completed_keyword(
    keyword: &str,
    item: &CompletionItem,
    ddl_on_relation_target: bool,
) -> bool {
    match keyword {
        "from" | "join" | "into" => is_relation_completion_kind(item.kind),
        "select" => !is_relation_completion_kind(item.kind),
        "on" if ddl_on_relation_target => is_relation_completion_kind(item.kind),
        "where" | "on" | "by" | "having" | "values" | "set" => {
            !is_relation_completion_kind(item.kind)
                && item.kind != Some(CompletionItemKind::OPERATOR)
        }
        "limit" | "offset" => true,
        _ => true,
    }
}

pub(super) fn is_relation_completion_kind(kind: Option<CompletionItemKind>) -> bool {
    matches!(
        kind,
        Some(
            CompletionItemKind::CLASS
                | CompletionItemKind::STRUCT
                | CompletionItemKind::MODULE
                | CompletionItemKind::FILE
                | CompletionItemKind::FOLDER
        )
    )
}

pub(super) fn deduplicate_simple_completion_items(items: &mut Vec<CompletionItem>) {
    let mut seen = HashSet::new();
    items.retain(|item| {
        let family = match item.kind {
            Some(CompletionItemKind::KEYWORD) => "keyword",
            Some(CompletionItemKind::OPERATOR) => "operator",
            _ => return true,
        };
        seen.insert(format!("{family}:{}", item.label.to_ascii_lowercase()))
    });
}

pub(super) fn next_snippet_tabstop_index(value: &str) -> usize {
    let bytes = value.as_bytes();
    let mut largest = 0usize;
    let mut cursor = 0usize;

    while cursor + 1 < bytes.len() {
        if bytes[cursor] != b'$' {
            cursor += 1;
            continue;
        }

        let mut digit_start = cursor + 1;
        let braced = bytes[digit_start] == b'{';
        if braced {
            digit_start += 1;
        }
        let mut digit_end = digit_start;
        while digit_end < bytes.len() && bytes[digit_end].is_ascii_digit() {
            digit_end += 1;
        }
        if digit_end == digit_start
            || (braced
                && !bytes
                    .get(digit_end)
                    .is_some_and(|character| matches!(*character, b':' | b'}' | b'|')))
        {
            cursor += 1;
            continue;
        }
        if let Ok(index) = value[digit_start..digit_end].parse::<usize>() {
            largest = largest.max(index);
        }
        cursor = digit_end;
    }

    largest.saturating_add(1).max(1)
}
