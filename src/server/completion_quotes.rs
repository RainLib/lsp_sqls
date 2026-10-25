//! Column metadata names are atoms, even when they contain dots. Build a
//! dialect-correct edit and replace the complete member, including its suffix.
use super::completion_context::position_to_byte_offset;
use super::completion_identifiers::{
    byte_offset_to_lsp_position, is_identifier_path_boundary, quote_raw_completion_identifier,
};
use crate::parser::SqlParser;
use tower_lsp::lsp_types::*;

pub(super) fn apply_column_identifier_edits(
    text: &str,
    position: Position,
    dialect: &str,
    items: &mut [CompletionItem],
) {
    if !items
        .iter()
        .any(|item| item.kind == Some(CompletionItemKind::FIELD) && item.text_edit.is_none())
    {
        return;
    }
    let offset = position_to_byte_offset(text, position);
    let start = SqlParser::identifier_path_token_for_dialect(
        &text[..offset],
        crate::placeholder::SqlPlaceholderDialect::for_name(Some(dialect)),
    )
    .map(|token| offset - token.len())
    .unwrap_or(offset);
    let member = member_start(&text[start..offset]) + start;
    let end = member_end(text, member, offset, dialect).max(offset);
    let path_start = byte_offset_to_lsp_position(text, start);
    let member_start = byte_offset_to_lsp_position(text, member);
    let member_end = byte_offset_to_lsp_position(text, end);
    for item in items {
        if item.kind != Some(CompletionItemKind::FIELD) || item.text_edit.is_some() {
            continue;
        }
        let name = item
            .data
            .as_ref()
            .and_then(|data| data.pointer("/oxide/column"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&item.label);
        let insert = item.insert_text.as_deref().unwrap_or(&item.label);
        let Some(prefix) = insert.strip_suffix(name) else {
            continue;
        };
        // A label may be qualified; only raw metadata or a bare field is safe.
        if !prefix.is_empty() && !prefix.ends_with('.') {
            continue;
        }
        let prefix = prefix
            .strip_suffix('.')
            .map(|prefix| {
                SqlParser::identifier_parts(prefix)
                    .iter()
                    .map(|part| quote_raw_completion_identifier(part, dialect))
                    .collect::<Vec<_>>()
                    .join(".")
                    + "."
            })
            .unwrap_or_default();
        let new_text = format!("{prefix}{}", quote_raw_completion_identifier(name, dialect));
        item.insert_text = Some(new_text.clone());
        item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: Range {
                start: if prefix.is_empty() {
                    member_start
                } else {
                    path_start
                },
                end: member_end,
            },
            new_text,
        }));
    }
}

fn member_start(path: &str) -> usize {
    let mut start = 0;
    let mut quote = None;
    let mut chars = path.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if let Some(close) = quote {
            if ch == close {
                if chars.peek().is_some_and(|(_, next)| *next == close) {
                    chars.next();
                } else {
                    quote = None;
                }
            }
        } else {
            match ch {
                '"' | '`' => quote = Some(ch),
                '[' => quote = Some(']'),
                '.' => start = index + 1,
                _ => {}
            }
        }
    }
    start
}
fn member_end(text: &str, start: usize, cursor: usize, dialect: &str) -> usize {
    let mut chars = text[start..].char_indices().peekable();
    let close = match chars.peek().map(|(_, ch)| *ch) {
        Some('"') => Some('"'),
        Some('`') => Some('`'),
        Some('[') => Some(']'),
        _ => None,
    };
    if close.is_some() {
        chars.next();
    }
    while let Some((index, ch)) = chars.next() {
        if let Some(close) = close {
            if ch == close {
                if chars.peek().is_some_and(|(_, next)| *next == close) {
                    chars.next();
                } else {
                    return start + index + ch.len_utf8();
                }
            }
        } else if ch == '.'
            || is_identifier_path_boundary(ch)
            || dialect == "postgres" && matches!(ch, '[' | ']')
        {
            return start + index;
        }
    }
    if close.is_some() {
        cursor
    } else {
        text.len()
    }
}
