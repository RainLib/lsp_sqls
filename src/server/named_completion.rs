//! Live parameter names share invocation binding with signatures.
use super::{
    completion_context::{position_to_byte_offset, CompletionCandidates},
    document_structure::position_for_offset,
    limits::BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS,
    routine_context::routine_call_at_position,
    routine_identity::{read_parameter_name, RoutineIdentity},
    routine_parameters::{routine_kind_matches, RoutineParameters},
};
use crate::{
    parser::SqlParser,
    placeholder::SqlPlaceholderDialect,
    schema::{Schema, SchemaManager},
    sql_lexical::lexical_rules_for,
};
use std::collections::{HashMap, HashSet};
use tower_lsp::lsp_types::*;

pub(super) fn completions(
    text: &str,
    position: Position,
    dialect: &str,
    current: impl FnOnce() -> Option<Schema>,
    manager: &SchemaManager,
) -> CompletionCandidates {
    let sqlserver = super::execute_context::enabled(dialect);
    if !sqlserver && lexical_rules_for(dialect).placeholder != SqlPlaceholderDialect::Postgres {
        return CompletionCandidates::default();
    }
    let Some(call) =
        routine_call_at_position(text, position, dialect).filter(|call| call.active_group == 0)
    else {
        return CompletionCandidates::default();
    };
    completions_for_call(text, position, dialect, current, manager, call)
}

pub(super) fn completions_for_call(
    text: &str,
    position: Position,
    dialect: &str,
    current: impl FnOnce() -> Option<Schema>,
    manager: &SchemaManager,
    mut call: super::routine_context::RoutineCallContext,
) -> CompletionCandidates {
    let sqlserver = super::execute_context::enabled(dialect);
    if sqlserver && !call.procedure_call {
        return CompletionCandidates::default();
    }
    let active = call.active_parameter as usize;
    if call.supplied.values.is_empty() && active == 0 {
        if let Some(range) = call.arguments.last() {
            call.supplied
                .values
                .push(super::routine_arguments::argument(
                    text,
                    range.clone(),
                    dialect,
                ));
        }
    }
    let Some(argument) = call.supplied.values.get(active) else {
        return CompletionCandidates::default();
    };
    let cursor = position_to_byte_offset(text, position);
    if !argument.full.contains(&cursor) && cursor != argument.full.end {
        return CompletionCandidates::default();
    }
    let content =
        SqlParser::mask_sql_operand_context_for_dialect(&text[argument.full.clone()], dialect);
    let mut start = argument.full.start
        + content
            .find(|ch: char| !ch.is_whitespace())
            .unwrap_or(content.len());
    if argument.explicit {
        start += 8;
        start += text[start..cursor.max(start)]
            .find(|ch: char| !ch.is_whitespace())
            .unwrap_or(cursor.saturating_sub(start));
    }
    if start > cursor {
        return CompletionCandidates::default();
    }
    let raw = &text[start..argument.full.end];
    let quoted = raw.starts_with('"');
    let mut end = if let Some((_, end)) = read_parameter_name(raw, dialect) {
        start + end
    } else if quoted {
        cursor
            + text[cursor..argument.full.end]
                .find(['\r', '\n', ';'])
                .unwrap_or(argument.full.end - cursor)
    } else if cursor == start {
        start
    } else {
        return CompletionCandidates::default();
    };
    if cursor > end {
        return CompletionCandidates::default();
    }
    let prefix = &text[start..cursor];
    let prefix = if quoted {
        let prefix = prefix.strip_prefix('"').unwrap_or("");
        let prefix = if cursor == end {
            prefix.strip_suffix('"').unwrap_or(prefix)
        } else {
            prefix
        };
        prefix.replace("\"\"", "\"")
    } else if prefix.chars().all(SqlParser::is_identifier_char) {
        prefix.to_ascii_lowercase()
    } else {
        return CompletionCandidates::default();
    };
    let tail =
        SqlParser::mask_sql_operand_context_for_dialect(&text[end..argument.full.end], dialect);
    let tail = tail.trim_start();
    let existing = if sqlserver {
        tail.starts_with('=') && !tail.starts_with("==")
    } else {
        tail.starts_with("=>") || tail.starts_with(":=")
    };
    if !existing && !tail.is_empty() {
        return CompletionCandidates::default();
    }
    end = end.max(cursor);
    let Some(identity) = RoutineIdentity::parse(&call.name, dialect) else {
        return CompletionCandidates::default();
    };
    let current = current();
    let Some(schema) = identity.schema(current.as_ref(), manager, dialect) else {
        return CompletionCandidates::default();
    };
    let mut candidates = Vec::new();
    for function in schema.functions.iter().filter(|function| {
        identity.matches(function, dialect) && routine_kind_matches(function, call.procedure_call)
    }) {
        let parameters = RoutineParameters::new(function, dialect, call.procedure_call);
        let available = call.supplied.available_named(&parameters, dialect, active);
        if !available.valid {
            continue;
        }
        let mut name_counts = HashMap::new();
        for parameter in parameters.iter() {
            *name_counts.entry(parameter.name.as_str()).or_insert(0usize) += 1;
        }
        for (index, parameter) in parameters.iter().enumerate() {
            if sqlserver && !crate::tsql_identifier::is_complete_name(&parameter.name) {
                continue;
            }
            if parameter.name.is_empty() || parameter.name.contains('\0') {
                continue;
            }
            if available.used.contains(&index) || name_counts[parameter.name.as_str()] != 1 {
                continue;
            }
            if argument.explicit {
                if parameters.variadic_index() != Some(index)
                    || active + 1 != call.supplied.values.len()
                {
                    continue;
                }
            } else if parameters.variadic_index() == Some(index) {
                continue;
            }
            let matches = if quoted {
                parameter.name.starts_with(&prefix)
            } else {
                parameter.name.to_ascii_lowercase().starts_with(&prefix)
            };
            if matches {
                candidates.push((*parameter, function));
            }
        }
    }
    candidates.sort_by_cached_key(|(parameter, _)| {
        (
            parameter.name.to_ascii_lowercase() != prefix,
            parameter.name.to_ascii_lowercase(),
        )
    });
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    let mut is_incomplete = false;
    for (parameter, function) in candidates {
        let key = (
            &parameter.name,
            &parameter.data_type,
            parameter.optional,
            &parameter.mode,
        );
        if !seen.insert(key) {
            continue;
        }
        if items.len() >= BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS {
            is_incomplete = true;
            break;
        }
        let name = if sqlserver {
            parameter.name.clone()
        } else {
            format!("\"{}\"", parameter.name.replace('"', "\"\""))
        };
        let new_text = if existing {
            name
        } else {
            format!(
                "{} {} $0",
                name.replace('\\', "\\\\")
                    .replace('$', "\\$")
                    .replace('}', "\\}"),
                if sqlserver { "=" } else { "=>" }
            )
        };
        items.push(CompletionItem {
            label: parameter.name.clone(),
            kind: Some(CompletionItemKind::FIELD),
            detail: Some(format!("Parameter: {}", parameter.signature_label())),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: function.markdown_documentation(),
            })),
            sort_text: Some(format!("0:parameter:{}", parameter.name)),
            filter_text: Some(parameter.name.clone()),
            insert_text: Some(new_text.clone()),
            insert_text_format: Some(if existing {
                InsertTextFormat::PLAIN_TEXT
            } else {
                InsertTextFormat::SNIPPET
            }),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: Range {
                    start: position_for_offset(text, start),
                    end: position_for_offset(text, end),
                },
                new_text,
            })),
            ..Default::default()
        });
    }
    CompletionCandidates {
        constraints: None,
        value_type: None,
        value_role: None,
        logical_continuation: None,
        pattern_escape: None,
        items,
        is_incomplete,
    }
}
