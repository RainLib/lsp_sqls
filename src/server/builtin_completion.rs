//! Builtin completion for the SQL language server.
use super::completion_context::{is_relation_completion_kind, position_to_byte_offset};
use super::completion_identifiers::identifier_path_start_before_cursor;
use super::limits::{BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS, SIGNATURE_HELP_MAX_SCAN_BYTES};
use crate::builtin_signatures::builtin_function_catalog_is_available_for;
use crate::builtin_signatures::builtin_function_is_known_for;
use crate::builtin_signatures::builtin_signature_completion_catalog_for;
use crate::builtin_signatures::builtin_table_signature_completion_catalog_for;
use crate::builtin_signatures::builtin_value_catalog_for;
use crate::builtin_signatures::builtin_window_function_catalog_is_available_for;
use crate::builtin_signatures::BuiltinSignature;
use crate::parser::SqlParser;
use crate::position::lsp_position_at_end;
use crate::schema::Schema;
use std::collections::HashSet;
use tower_lsp::lsp_types::*;
pub(super) fn builtin_function_context(text: &str, position: Position) -> Option<String> {
    let offset = position_to_byte_offset(text, position).min(text.len());
    let mut scan_start = offset.saturating_sub(SIGNATURE_HELP_MAX_SCAN_BYTES);
    while scan_start < offset && !text.is_char_boundary(scan_start) {
        scan_start += 1;
    }
    let window = text.get(scan_start..offset)?;
    let masked = SqlParser::mask_sql_noise(window);
    if let Some((last_non_whitespace, character)) = window
        .char_indices()
        .rev()
        .find(|(_, character)| !character.is_whitespace())
    {
        let masked_character = masked[last_non_whitespace..].chars().next()?;
        if !character.is_whitespace() && masked_character == ' ' {
            return None;
        }
    }

    let context = SqlParser::completion_context_from_text(window, lsp_position_at_end(window));
    if !matches!(
        context,
        crate::parser::CompletionContext::SelectClause
            | crate::parser::CompletionContext::WhereClause
            | crate::parser::CompletionContext::ExpressionValueClause
            | crate::parser::CompletionContext::CaseResultClause
            | crate::parser::CompletionContext::JoinConditionClause
            | crate::parser::CompletionContext::OrderByClause
            | crate::parser::CompletionContext::GroupByClause
            | crate::parser::CompletionContext::HavingClause
            | crate::parser::CompletionContext::InsertValueClause
    ) {
        return None;
    }

    let prefix_start = identifier_path_start_before_cursor(window).unwrap_or(window.len());
    let raw_prefix = window.get(prefix_start..)?.trim();
    if raw_prefix.contains('.')
        || raw_prefix.starts_with(['"', '\'', '`', '['])
        || raw_prefix.ends_with([']', '"', '`'])
    {
        return None;
    }
    Some(SqlParser::identifier_last_part(raw_prefix).to_ascii_lowercase())
}

pub(super) fn relation_function_context(text: &str, position: Position) -> Option<String> {
    let offset = position_to_byte_offset(text, position).min(text.len());
    let mut scan_start = offset.saturating_sub(SIGNATURE_HELP_MAX_SCAN_BYTES);
    while scan_start < offset && !text.is_char_boundary(scan_start) {
        scan_start += 1;
    }
    let window = text.get(scan_start..offset)?;
    let masked = SqlParser::mask_sql_noise(window);
    if let Some((last_non_whitespace, character)) = window
        .char_indices()
        .rev()
        .find(|(_, character)| !character.is_whitespace())
    {
        let masked_character = masked[last_non_whitespace..].chars().next()?;
        if !character.is_whitespace() && masked_character == ' ' {
            return None;
        }
    }

    let context = SqlParser::completion_context_from_text(window, lsp_position_at_end(window));
    if !matches!(
        context,
        crate::parser::CompletionContext::FromClause | crate::parser::CompletionContext::JoinClause
    ) {
        return None;
    }

    let prefix_start = identifier_path_start_before_cursor(window).unwrap_or(window.len());
    let raw_prefix = window.get(prefix_start..)?.trim();
    if raw_prefix.contains('.')
        || raw_prefix.starts_with(['"', '\'', '`', '['])
        || raw_prefix.ends_with([']', '"', '`'])
    {
        return None;
    }
    Some(SqlParser::identifier_last_part(raw_prefix).to_ascii_lowercase())
}

pub(super) fn snippet_placeholder_label(parameter: &str) -> String {
    parameter
        .trim()
        .trim_start_matches("...")
        .trim_end_matches('?')
        .replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('}', "\\}")
}

pub(super) fn builtin_function_snippet(signature: &BuiltinSignature) -> String {
    let mut placeholder = 1usize;
    let groups = signature
        .parameter_groups
        .iter()
        .map(|parameters| {
            let parameters = parameters
                .iter()
                .map(|parameter| {
                    let label = snippet_placeholder_label(parameter);
                    let value = format!("${{{placeholder}:{label}}}");
                    placeholder += 1;
                    value
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("({parameters})")
        })
        .collect::<String>();
    format!("{}{groups}", signature.name)
}

pub(super) fn completion_item_is_live_routine(item: &CompletionItem) -> bool {
    item.kind == Some(CompletionItemKind::FUNCTION)
        && item.detail.as_deref().is_some_and(|detail| {
            detail.starts_with("Function:") || detail.starts_with("Procedure:")
        })
}

pub(super) fn add_builtin_function_completions(
    text: &str,
    position: Position,
    dialect: &str,
    schema: Option<&Schema>,
    items: &mut Vec<CompletionItem>,
) {
    let Some(prefix) = builtin_function_context(text, position) else {
        return;
    };
    let server_version = schema.and_then(Schema::server_version_tuple);
    let mut catalog = builtin_signature_completion_catalog_for(
        dialect,
        server_version,
        &prefix,
        BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS,
    );
    let full_values = builtin_value_catalog_for(dialect);
    if catalog.is_empty()
        && full_values.is_empty()
        && !builtin_function_catalog_is_available_for(dialect)
    {
        return;
    }
    let authoritative_value_names = full_values
        .iter()
        .map(|value| value.name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut values = full_values
        .into_iter()
        .filter(|value| prefix.is_empty() || value.name.to_ascii_lowercase().starts_with(&prefix))
        .take(BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS.saturating_sub(catalog.len()))
        .collect::<Vec<_>>();

    let live_names = items
        .iter()
        .filter(|item| completion_item_is_live_routine(item))
        .map(|item| SqlParser::identifier_last_part(&item.label).to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let catalog_names = catalog
        .iter()
        .map(|signature| signature.name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let value_names = values
        .iter()
        .map(|value| value.name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    items.retain(|item| {
        let name = SqlParser::identifier_last_part(&item.label).to_ascii_lowercase();
        if completion_item_is_live_routine(item) {
            return true;
        }
        if item.kind == Some(CompletionItemKind::FUNCTION)
            && !builtin_function_is_known_for(dialect, &name, server_version)
        {
            return false;
        }
        if catalog_names.contains(&name) || value_names.contains(&name) {
            return false;
        }
        !authoritative_value_names.contains(&name)
    });

    catalog.retain(|signature| !live_names.contains(&signature.name.to_ascii_lowercase()));
    values.retain(|value| !live_names.contains(&value.name.to_ascii_lowercase()));
    items.extend(catalog.into_iter().map(|signature| CompletionItem {
        label: signature.name.to_string(),
        kind: Some(CompletionItemKind::FUNCTION),
        detail: Some(format!("Built-in function: {}", signature.label())),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!(
                "Built-in function signature for the active `{dialect}` SQL profile. Live routine metadata takes precedence when available."
            ),
        })),
        sort_text: Some(format!(
            "1:builtin:{}:{}",
            signature.name.to_ascii_lowercase(),
            signature.label().to_ascii_lowercase()
        )),
        filter_text: Some(if signature
            .name
            .to_ascii_lowercase()
            .starts_with(&prefix)
        {
            signature.name.to_string()
        } else {
            format!("{prefix} {}", signature.name)
        }),
        insert_text: Some(builtin_function_snippet(&signature)),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }));
    items.extend(values.into_iter().map(|value| CompletionItem {
        label: value.name.to_string(),
        kind: Some(CompletionItemKind::VALUE),
        detail: Some(value.category.to_string()),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!(
                "Built-in system value for the active `{dialect}` SQL profile. It is inserted as a bare expression without function-call parentheses."
            ),
        })),
        sort_text: Some(format!("1:builtin-value:{}", value.name.to_ascii_lowercase())),
        filter_text: Some(value.name.to_string()),
        insert_text: Some(value.name.to_string()),
        insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
        ..Default::default()
    }));
}

pub(super) fn add_clickhouse_table_function_completions(
    text: &str,
    position: Position,
    dialect: &str,
    items: &mut Vec<CompletionItem>,
) {
    let Some(prefix) = relation_function_context(text, position) else {
        return;
    };
    let catalog = builtin_table_signature_completion_catalog_for(
        dialect,
        &prefix,
        BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS,
    );
    if catalog.is_empty() {
        return;
    }
    let existing_relations = items
        .iter()
        .filter(|item| is_relation_completion_kind(item.kind))
        .map(|item| item.label.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    items.extend(catalog.into_iter().filter_map(|signature| {
        if existing_relations.contains(&signature.name.to_ascii_lowercase()) {
            return None;
        }
        Some(CompletionItem {
            label: signature.name.to_string(),
            kind: Some(CompletionItemKind::CLASS),
            detail: Some(format!("ClickHouse table function: {}", signature.label())),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "Static ClickHouse table-function signature aligned with the DBX catalog."
                    .to_string(),
            })),
            sort_text: Some(format!(
                "1:table-function:{}:{}",
                signature.name.to_ascii_lowercase(),
                signature.label().to_ascii_lowercase()
            )),
            filter_text: Some(signature.name.to_string()),
            insert_text: Some(builtin_function_snippet(&signature)),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        })
    }));
}

pub(super) fn is_oracle_like_dialect(dialect: &str) -> bool {
    matches!(
        dialect.to_ascii_lowercase().as_str(),
        "oracle" | "oceanbase-oracle"
    )
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct OracleTableRoutineContext {
    pub(super) prefix: String,
    pub(super) qualifier: Option<String>,
}

pub(super) fn previous_ascii_word(text: &str) -> (&str, &str) {
    let trimmed = text.trim_end();
    let start = trimmed
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            (!character.is_ascii_alphanumeric() && character != '_')
                .then_some(index + character.len_utf8())
        })
        .unwrap_or(0);
    (&trimmed[..start], &trimmed[start..])
}

pub(super) fn oracle_table_routine_context(
    text: &str,
    position: Position,
) -> Option<OracleTableRoutineContext> {
    let offset = position_to_byte_offset(text, position).min(text.len());
    let mut scan_start = offset.saturating_sub(SIGNATURE_HELP_MAX_SCAN_BYTES);
    while scan_start < offset && !text.is_char_boundary(scan_start) {
        scan_start += 1;
    }
    let window = text.get(scan_start..offset)?;
    let masked = SqlParser::mask_sql_noise(window);
    let path_start = identifier_path_start_before_cursor(window).unwrap_or(window.len());
    let raw_path = window.get(path_start..)?.trim();
    let masked_path = masked.get(path_start..)?.trim();
    if raw_path.is_empty()
        || masked_path.is_empty()
        || raw_path.starts_with(['\'', '`', '['])
        || raw_path.ends_with([']', '`'])
    {
        return None;
    }

    let before_path = masked.get(..path_start)?.trim_end();
    let before_open = before_path.strip_suffix('(')?.trim_end();
    let (before_table, table_word) = previous_ascii_word(before_open);
    if !table_word.eq_ignore_ascii_case("table") {
        return None;
    }
    let (_, relation_word) = previous_ascii_word(before_table);
    if !matches!(relation_word.to_ascii_lowercase().as_str(), "from" | "join") {
        return None;
    }

    let (qualifier, prefix) = raw_path
        .rsplit_once('.')
        .map(|(qualifier, prefix)| (Some(qualifier.trim().to_string()), prefix))
        .unwrap_or((None, raw_path));
    if qualifier
        .as_ref()
        .is_some_and(|qualifier| qualifier.is_empty())
    {
        return None;
    }
    Some(OracleTableRoutineContext {
        prefix: SqlParser::identifier_last_part(prefix).to_ascii_lowercase(),
        qualifier,
    })
}

pub(super) fn add_oracle_table_function_completions(
    text: &str,
    position: Position,
    dialect: &str,
    schema: Option<&Schema>,
    items: &mut Vec<CompletionItem>,
) {
    if !is_oracle_like_dialect(dialect) {
        return;
    }

    if let Some(context) = oracle_table_routine_context(text, position) {
        items.clear();
        let Some(schema) = schema else {
            return;
        };
        let existing = items
            .iter()
            .map(|item| item.label.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        items.extend(schema.functions.iter().filter_map(|function| {
            if function
                .routine_type
                .as_deref()
                .is_some_and(|routine_type| routine_type.eq_ignore_ascii_case("procedure"))
            {
                return None;
            }
            let simple_name = SqlParser::identifier_last_part(&function.name);
            if !context.prefix.is_empty()
                && !simple_name
                    .to_ascii_lowercase()
                    .starts_with(&context.prefix)
            {
                return None;
            }
            if existing.contains(&simple_name.to_ascii_lowercase()) {
                return None;
            }
            let apply_name = context
                .qualifier
                .as_ref()
                .map(|qualifier| format!("{qualifier}.{simple_name}"))
                .unwrap_or_else(|| function.name.clone());
            let arguments = function
                .parameters
                .iter()
                .enumerate()
                .map(|(index, parameter)| {
                    let label = if parameter.name.trim().is_empty() {
                        parameter.data_type.as_str()
                    } else {
                        parameter.name.as_str()
                    };
                    format!("${{{}:{}}}", index + 1, snippet_placeholder_label(label))
                })
                .collect::<Vec<_>>()
                .join(", ");
            Some(CompletionItem {
                label: simple_name.to_string(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(format!(
                    "Oracle table function: {} -> {}",
                    function.signature(),
                    function.return_type
                )),
                documentation: Some(Documentation::String(function.documentation())),
                sort_text: Some(format!(
                    "0:oracle-table-routine:{}",
                    simple_name.to_ascii_lowercase()
                )),
                filter_text: Some(simple_name.to_string()),
                insert_text: Some(format!("{apply_name}({arguments})")),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                ..Default::default()
            })
        }));
        return;
    }

    let Some(prefix) = relation_function_context(text, position) else {
        return;
    };
    const ORACLE_TABLE_FUNCTIONS: &[(&str, &[&str], &str)] = &[
        ("TABLE", &["function_call"], "Oracle table function"),
        ("THE", &["subquery"], "Oracle nested-table expression"),
        ("XMLTABLE", &["xpath"], "XML to relational rows"),
        ("JSON_TABLE", &["expr", "path"], "JSON to relational rows"),
    ];
    let existing_relations = items
        .iter()
        .filter(|item| is_relation_completion_kind(item.kind))
        .map(|item| item.label.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    items.extend(
        ORACLE_TABLE_FUNCTIONS
            .iter()
            .filter(|(name, _, _)| {
                prefix.is_empty() || name.to_ascii_lowercase().starts_with(&prefix)
            })
            .filter_map(|(name, parameters, detail)| {
                if existing_relations.contains(&name.to_ascii_lowercase()) {
                    return None;
                }
                let arguments = parameters
                    .iter()
                    .enumerate()
                    .map(|(index, parameter)| format!("${{{}:{parameter}}}", index + 1))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(CompletionItem {
                    label: (*name).to_string(),
                    kind: Some(CompletionItemKind::CLASS),
                    detail: Some((*detail).to_string()),
                    documentation: Some(Documentation::String(format!(
                        "Oracle-family relation-source block for `{name}`."
                    ))),
                    sort_text: Some(format!(
                        "1:oracle-table-function:{}",
                        name.to_ascii_lowercase()
                    )),
                    filter_text: Some((*name).to_string()),
                    insert_text: Some(format!("{name}({arguments})")),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    ..Default::default()
                })
            }),
    );
}

pub(super) fn add_window_function_completions(
    text: &str,
    position: Position,
    dialect: &str,
    items: &mut Vec<CompletionItem>,
) {
    if !builtin_window_function_catalog_is_available_for(dialect) {
        return;
    }
    let Some(prefix) = builtin_function_context(text, position) else {
        return;
    };
    const WINDOW_FUNCTIONS: &[(&str, &[&str])] = &[
        ("ROW_NUMBER", &[]),
        ("RANK", &[]),
        ("DENSE_RANK", &[]),
        ("LAG", &["value"]),
        ("LEAD", &["value"]),
        ("FIRST_VALUE", &["value"]),
        ("LAST_VALUE", &["value"]),
        ("NTILE", &["buckets"]),
    ];
    let candidates = WINDOW_FUNCTIONS
        .iter()
        .filter(|(name, _)| prefix.is_empty() || name.to_ascii_lowercase().starts_with(&prefix))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return;
    }
    let candidate_names = candidates
        .iter()
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let live_names = items
        .iter()
        .filter(|item| completion_item_is_live_routine(item))
        .map(|item| SqlParser::identifier_last_part(&item.label).to_ascii_lowercase())
        .collect::<HashSet<_>>();
    items.retain(|item| {
        let name = SqlParser::identifier_last_part(&item.label).to_ascii_lowercase();
        completion_item_is_live_routine(item)
            || !matches!(
                item.kind,
                Some(CompletionItemKind::FUNCTION | CompletionItemKind::KEYWORD)
            )
            || !candidate_names.contains(&name)
    });

    items.extend(candidates.into_iter().filter_map(|(name, parameters)| {
        if live_names.contains(&name.to_ascii_lowercase()) {
            return None;
        }
        let mut placeholder = 1usize;
        let arguments = parameters
            .iter()
            .map(|parameter| {
                let argument = format!("${{{placeholder}:{parameter}}}");
                placeholder += 1;
                argument
            })
            .collect::<Vec<_>>()
            .join(", ");
        let partition = placeholder;
        let order = placeholder + 1;
        Some(CompletionItem {
            label: (*name).to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some("Window function with OVER clause".to_string()),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!(
                    "Portable window-function block for the active `{dialect}` SQL profile. Edit or remove the partition expression as needed."
                ),
            })),
            sort_text: Some(format!("1:window-function:{}", name.to_ascii_lowercase())),
            filter_text: Some((*name).to_string()),
            insert_text: Some(format!(
                "{name}({arguments}) OVER (PARTITION BY ${{{partition}:column}} ORDER BY ${{{order}:column}})"
            )),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        })
    }));
}
