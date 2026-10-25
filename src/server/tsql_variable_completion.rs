//! Typed @-prefix completions for scalar expression sites, distinct from EXEC
//! named arguments/targets and object or declaration-name positions.
use super::{
    completion_context::{position_to_byte_offset, CompletionCandidates},
    document_structure::position_for_offset,
    execute_context::{enabled, keyword_at},
    limits::BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS,
};
use crate::{
    parser::{CompletionContext, SqlParser},
    tsql_identifier,
};
use tower_lsp::lsp_types::*;

pub(super) fn completions(
    text: &str,
    position: Position,
    dialect: &str,
) -> Option<CompletionCandidates> {
    if !enabled(dialect) || !text.as_bytes().contains(&b'@') {
        return None;
    }
    let cursor = position_to_byte_offset(text, position);
    // Ordinary non-variable requests do not allocate lexical masks or parse.
    let mut start = cursor;
    let mut units = 0;
    for (at, ch) in text[..cursor].char_indices().rev() {
        if !tsql_identifier::is_name_character(ch) {
            break;
        }
        units += ch.len_utf16();
        if units > tsql_identifier::MAX_NAME_UTF16_UNITS {
            return None;
        }
        start = at;
    }
    let prefix = &text[start..cursor];
    if !prefix.starts_with('@') || prefix.starts_with("@@") {
        return None;
    }
    let end = tsql_identifier::prefix_end(text, start)?;
    let noise = SqlParser::mask_sql_noise_for_dialect(text, dialect);
    let content = SqlParser::mask_sql_operand_context_for_dialect(text, dialect);
    let before = content[..start].trim_end();
    if before.ends_with('.') {
        return Some(CompletionCandidates::default());
    }
    let word = before
        .rsplit(|ch: char| !SqlParser::is_identifier_char(ch))
        .next()
        .unwrap_or_default();
    if [
        "DECLARE",
        "PROC",
        "PROCEDURE",
        "FUNCTION",
        "FROM",
        "JOIN",
        "INTO",
        "UPDATE",
        "TABLE",
        "RETURNS",
        "AS",
        "EXEC",
        "EXECUTE",
    ]
    .iter()
    .any(|name| word.eq_ignore_ascii_case(name))
    {
        return Some(CompletionCandidates::default());
    }
    let variables = super::tsql_variables::declared(text, &noise, &content, cursor);
    if variables.scope_start > start {
        return Some(CompletionCandidates {
            constraints: None,
            value_type: None,
            value_role: None,
            logical_continuation: None,
            pattern_escape: None,
            is_incomplete: variables.is_incomplete,
            ..Default::default()
        });
    }
    let scoped = &text[variables.scope_start..];
    let scoped_position = position_for_offset(scoped, cursor - variables.scope_start);
    let context = SqlParser::completion_context_from_text(scoped, scoped_position);
    let expression = matches!(
        context,
        CompletionContext::SelectClause
            | CompletionContext::WhereClause
            | CompletionContext::ExpressionValueClause
            | CompletionContext::CaseResultClause
            | CompletionContext::OrderByClause
            | CompletionContext::GroupByClause
            | CompletionContext::HavingClause
            | CompletionContext::JoinConditionClause
            | CompletionContext::InsertValueClause
    );
    // RETURN and SET names are module statements outside the generic SELECT
    // context classifier. A named EXEC argument remains owned by its scanner.
    let statement = content[..start]
        .rfind(';')
        .map_or(0, |at| at + 1)
        .max(variables.scope_start);
    let return_expression = context == CompletionContext::Default
        && noise[statement..start]
            .char_indices()
            .any(|(at, _)| keyword_at(&noise, statement + at, "RETURN"));
    let explicit = return_expression
        || word.eq_ignore_ascii_case("RETURN")
        || word.eq_ignore_ascii_case("SET");
    if !expression && !explicit {
        return Some(CompletionCandidates::default());
    }
    let first = statement
        + content[statement..start]
            .find(|ch: char| !ch.is_whitespace())
            .unwrap_or(start - statement);
    if keyword_at(&noise, first, "EXEC") || keyword_at(&noise, first, "EXECUTE") {
        return None;
    }
    let prefix = prefix.to_ascii_lowercase();
    let range = Range::new(
        position_for_offset(text, start),
        position_for_offset(text, end),
    );
    let mut items: Vec<_> = variables
        .variables
        .into_iter()
        .filter(|variable| {
            variable.scalar && variable.name.to_ascii_lowercase().starts_with(&prefix)
        })
        .map(|variable| {
            let lower = variable.name.to_ascii_lowercase();
            CompletionItem {
                label: variable.name.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: Some(format!(
                    "Variable: {} ({})",
                    variable.name, variable.data_type
                )),
                sort_text: Some(format!("1:variable:{}:{lower}", u8::from(lower != prefix))),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: variable.name,
                })),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                ..Default::default()
            }
        })
        .collect();
    items.sort_by(|a, b| a.sort_text.cmp(&b.sort_text));
    let is_incomplete =
        variables.is_incomplete || items.len() > BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS;
    items.truncate(BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS);
    Some(CompletionCandidates {
        constraints: None,
        value_type: None,
        value_role: None,
        logical_continuation: None,
        pattern_escape: None,
        items,
        is_incomplete,
    })
}
