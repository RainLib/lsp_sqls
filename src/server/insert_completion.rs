//! Insert completion for the SQL language server.
use super::completion_context::position_to_byte_offset;
use super::completion_identifiers::{keyword_with_completion_case, quote_completion_identifier};
use super::diagnostics::contains_sql_keyword;
use super::preferences::CompletionPreferences;
use super::schema_resolution::schema_table_matches;
use crate::parser::SqlParser;
use crate::position::lsp_position_at_end;
use crate::schema::Column;
use crate::schema::Schema;
use crate::schema::Table;
use tower_lsp::lsp_types::*;
pub(super) const INSERT_STATEMENT_SNIPPET_MAX_TABLES: usize = 32;

pub(super) const INSERT_STATEMENT_SNIPPET_MAX_COLUMNS: usize = 64;

pub(super) fn add_insert_statement_snippets(
    text: &str,
    position: Position,
    schema: Option<&Schema>,
    items: &mut Vec<CompletionItem>,
    preferences: &CompletionPreferences,
    dialect_name: &str,
) {
    let Some(schema) = schema else {
        return;
    };
    let cursor = position_to_byte_offset(text, position);
    let Some(text_before_cursor) = text.get(..cursor) else {
        return;
    };
    let statement_start = SqlParser::active_statement_start(text_before_cursor);
    let fragment = &text_before_cursor[statement_start..];
    if fragment.chars().last().is_some_and(char::is_whitespace) {
        return;
    }
    let prefix = fragment.trim().to_ascii_lowercase();
    if prefix.len() < 2
        || !prefix
            .chars()
            .all(|character| character.is_ascii_alphabetic())
        || !"insert".starts_with(&prefix)
    {
        return;
    }

    let insert = keyword_with_completion_case("INSERT", preferences.keyword_case);
    let into = keyword_with_completion_case("INTO", preferences.keyword_case);
    let values = keyword_with_completion_case("VALUES", preferences.keyword_case);
    let schema_name = (!schema.database.trim().is_empty())
        .then(|| quote_completion_identifier(schema.database.trim(), dialect_name));
    let mut tables = schema.tables.iter().collect::<Vec<_>>();
    tables.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });

    let mut emitted_tables = 0usize;
    for table in tables {
        if table
            .object_type
            .as_deref()
            .is_some_and(|kind| kind.to_ascii_lowercase().contains("view"))
        {
            continue;
        }
        let writable = table
            .columns
            .iter()
            .filter(|column| !column.auto_increment && !column.generated && !column.hidden)
            .collect::<Vec<_>>();
        if writable.is_empty() || writable.len() > INSERT_STATEMENT_SNIPPET_MAX_COLUMNS {
            continue;
        }
        if emitted_tables >= INSERT_STATEMENT_SNIPPET_MAX_TABLES {
            break;
        }
        emitted_tables += 1;
        let required = writable
            .iter()
            .copied()
            .filter(|column| {
                !column.nullable
                    && column
                        .default_value
                        .as_deref()
                        .is_none_or(|value| value.trim().is_empty())
            })
            .collect::<Vec<_>>();
        if !required.is_empty() && required.len() != writable.len() {
            push_insert_statement_snippet(
                items,
                table,
                schema_name.as_deref(),
                &required,
                &insert,
                &into,
                &values,
                dialect_name,
                true,
            );
        }
        push_insert_statement_snippet(
            items,
            table,
            schema_name.as_deref(),
            &writable,
            &insert,
            &into,
            &values,
            dialect_name,
            false,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_insert_statement_snippet(
    items: &mut Vec<CompletionItem>,
    table: &Table,
    schema_name: Option<&str>,
    columns: &[&Column],
    insert: &str,
    into: &str,
    values: &str,
    dialect_name: &str,
    required_only: bool,
) {
    let quoted_table = quote_completion_identifier(&table.name, dialect_name);
    let relation = schema_name
        .map(|schema| format!("{schema}.{quoted_table}"))
        .unwrap_or(quoted_table);
    let quoted_columns = columns
        .iter()
        .map(|column| quote_completion_identifier(&column.name, dialect_name))
        .collect::<Vec<_>>();
    let column_lines = quoted_columns.join(",\n  ");
    let value_lines = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let placeholder = column
                .name
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() || character == '_' {
                        character
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            format!("${{{}:{placeholder}_value}}", index + 1)
        })
        .collect::<Vec<_>>()
        .join(",\n  ");
    let label = if required_only {
        format!("insert {} required", table.name)
    } else {
        format!("insert {}", table.name)
    };
    if items.iter().any(|item| item.label == label) {
        return;
    }
    items.push(CompletionItem {
        label,
        kind: Some(CompletionItemKind::SNIPPET),
        detail: Some(format!(
            "{} INSERT · {} writable column{}",
            if required_only {
                "Required"
            } else {
                "Complete"
            },
            columns.len(),
            if columns.len() == 1 { "" } else { "s" }
        )),
        filter_text: Some(format!("ins insert {}", table.name)),
        sort_text: Some(format!(
            "0:{}:{}",
            if required_only { 0 } else { 1 },
            table.name.to_ascii_lowercase()
        )),
        insert_text: Some(format!(
            "{insert} {into} {relation} (\n  {column_lines}\n)\n{values} (\n  {value_lines}\n);"
        )),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    });
}

pub(super) fn add_insert_all_columns_completion(
    text: &str,
    position: Position,
    schema: Option<&Schema>,
    items: &mut Vec<CompletionItem>,
    preferences: &CompletionPreferences,
    dialect_name: &str,
) {
    let Some(schema) = schema else {
        return;
    };
    let cursor = position_to_byte_offset(text, position);
    let Some(text_before_cursor) = text.get(..cursor) else {
        return;
    };
    let statement_start = SqlParser::active_statement_start(text_before_cursor);
    let statement = &text_before_cursor[statement_start..];
    let masked = SqlParser::mask_sql_noise(statement);
    let trimmed = masked.trim_end();
    if !trimmed.ends_with('(') {
        return;
    }
    let upper = trimmed.to_ascii_uppercase();
    let Some(insert_offset) = upper.rfind("INSERT INTO") else {
        return;
    };
    let open_offset = trimmed.len().saturating_sub(1);
    let insert_tail = &upper[insert_offset + "INSERT INTO".len()..open_offset];
    if ["VALUES", "VALUE", "SELECT", "SET"]
        .iter()
        .any(|keyword| contains_sql_keyword(insert_tail, keyword))
        || insert_tail
            .chars()
            .any(|character| matches!(character, '(' | ')' | ','))
    {
        return;
    }

    let mut parser = SqlParser::new();
    let parse_result = parser.parse(statement);
    let Some(tree) = parse_result.tree.as_ref() else {
        return;
    };
    let references = parser.extract_referenced_tables_at_position(
        tree,
        statement,
        SqlParser::lsp_position_to_byte_position(statement, lsp_position_at_end(statement)),
    );
    let Some(reference) = references.last() else {
        return;
    };
    let Some(table) = schema
        .tables
        .iter()
        .find(|table| schema_table_matches(schema, reference, table))
    else {
        return;
    };
    let writable_columns = table
        .columns
        .iter()
        .filter(|column| !column.auto_increment && !column.generated && !column.hidden)
        .collect::<Vec<_>>();
    if writable_columns.is_empty() {
        return;
    }

    let required_columns = writable_columns
        .iter()
        .copied()
        .filter(|column| {
            !column.nullable
                && column
                    .default_value
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
        })
        .collect::<Vec<_>>();

    if !required_columns.is_empty() && required_columns.len() != writable_columns.len() {
        add_insert_columns_completion_item(
            table,
            &format!("{}.required", table.name),
            "Required writable",
            &required_columns,
            items,
            preferences,
            dialect_name,
            0,
        );
    }
    add_insert_columns_completion_item(
        table,
        &format!("{}.*", table.name),
        "All writable",
        &writable_columns,
        items,
        preferences,
        dialect_name,
        1,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn add_insert_columns_completion_item(
    table: &Table,
    label: &str,
    detail_prefix: &str,
    source_columns: &[&Column],
    items: &mut Vec<CompletionItem>,
    preferences: &CompletionPreferences,
    dialect_name: &str,
    sort_rank: usize,
) {
    if source_columns.is_empty() || items.iter().any(|item| item.label == label) {
        return;
    }
    let columns = source_columns
        .iter()
        .map(|column| quote_completion_identifier(&column.name, dialect_name))
        .collect::<Vec<_>>();
    let values = columns
        .iter()
        .enumerate()
        .map(|(index, _)| format!("${{{}:value}}", index + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let values_keyword = keyword_with_completion_case("VALUES", preferences.keyword_case);
    let insert_text = format!("{}) {values_keyword} ({values})", columns.join(", "));
    items.push(CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::SNIPPET),
        detail: Some(format!(
            "{detail_prefix} · {} columns: {}) {} (value, ...)",
            columns.len(),
            columns.join(", "),
            values_keyword
        )),
        sort_text: Some(format!(
            "0:{sort_rank:04}:{}",
            table.name.to_ascii_lowercase()
        )),
        filter_text: Some(table.name.clone()),
        insert_text: Some(insert_text),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    });
}
