//! Code actions for the SQL language server.
use super::completion_context::position_to_byte_offset;
use super::completion_identifiers::{
    identifier_path_start_before_cursor, is_identifier_path_boundary, quote_completion_identifier,
};
use super::document_structure::range_for_offsets;
use super::schema_resolution::schema_table_matches;
use super::signature_help::single_document_edit;
use crate::parser::SqlParser;
use crate::schema::Schema;
use crate::schema::Table;
use crate::token::Keywords;
use std::collections::HashMap;
use std::collections::HashSet;
use tower_lsp::lsp_types::*;
pub(super) fn code_action_kind_explicitly_requested(
    context: &CodeActionContext,
    kind: &CodeActionKind,
) -> bool {
    context.only.as_ref().is_some_and(|only| {
        only.iter().any(|requested| {
            kind.as_str() == requested.as_str()
                || kind
                    .as_str()
                    .starts_with(&format!("{}.", requested.as_str()))
        })
    })
}

#[derive(Debug, Clone)]
pub(super) struct IdentifierPathSelection {
    pub(super) range: Range,
    pub(super) raw_parts: Vec<String>,
    pub(super) normalized_parts: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ColumnQualificationSource {
    pub(super) relation: String,
    pub(super) qualifier_label: String,
    pub(super) qualifier_sql: String,
    pub(super) columns: Vec<String>,
    pub(super) is_aliased: bool,
}

pub(super) fn identifier_path_end(text: &str, start: usize) -> usize {
    let mut quote_end: Option<char> = None;
    let mut index = start;
    while index < text.len() {
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if let Some(closing_quote) = quote_end {
            index += character.len_utf8();
            if character == closing_quote {
                if text[index..].starts_with(closing_quote) {
                    index += closing_quote.len_utf8();
                } else {
                    quote_end = None;
                }
            }
            continue;
        }
        match character {
            '"' | '`' => quote_end = Some(character),
            '[' => quote_end = Some(']'),
            _ if is_identifier_path_boundary(character) || character == ':' => break,
            _ => {}
        }
        index += character.len_utf8();
    }
    index
}

pub(super) fn split_identifier_path_sql(
    raw: &str,
    dialect_name: &str,
) -> Option<(Vec<String>, Vec<String>)> {
    let mut raw_parts = Vec::new();
    let mut normalized_parts = Vec::new();
    let mut index = 0;
    while index < raw.len() {
        let start = index;
        let first = raw[index..].chars().next()?;
        let (normalized, end) = if matches!(first, '"' | '`' | '[') {
            if first == '"' && matches!(dialect_name, "mysql" | "hive" | "clickhouse") {
                return None;
            }
            let closing = if first == '[' { ']' } else { first };
            let mut value = String::new();
            index += first.len_utf8();
            let mut closed = false;
            while index < raw.len() {
                let character = raw[index..].chars().next()?;
                index += character.len_utf8();
                if character == closing {
                    if raw[index..].starts_with(closing) {
                        value.push(closing);
                        index += closing.len_utf8();
                    } else {
                        closed = true;
                        break;
                    }
                } else {
                    value.push(character);
                }
            }
            if !closed || value.is_empty() {
                return None;
            }
            (value, index)
        } else {
            if !SqlParser::is_identifier_char(first) {
                return None;
            }
            index += first.len_utf8();
            while index < raw.len() {
                let character = raw[index..].chars().next()?;
                if !SqlParser::is_identifier_char(character) {
                    break;
                }
                index += character.len_utf8();
            }
            (raw[start..index].to_string(), index)
        };
        raw_parts.push(raw[start..end].to_string());
        normalized_parts.push(normalized);
        if index == raw.len() {
            break;
        }
        if !raw[index..].starts_with('.') {
            return None;
        }
        index += 1;
    }
    (!raw_parts.is_empty()).then_some((raw_parts, normalized_parts))
}

pub(super) fn identifier_path_selection(
    text: &str,
    request_range: Range,
    dialect_name: &str,
) -> Option<IdentifierPathSelection> {
    let request_start = position_to_byte_offset(text, request_range.start);
    let request_end = position_to_byte_offset(text, request_range.end);
    let cursor = request_end.max(request_start).min(text.len());
    let prefix = text.get(..cursor)?;
    let start = identifier_path_start_before_cursor(prefix)?;
    let end = identifier_path_end(text, start);
    if cursor < start || cursor > end || start >= end {
        return None;
    }
    let raw = text.get(start..end)?.trim();
    let trim_start = text.get(start..end)?.len() - text.get(start..end)?.trim_start().len();
    let start = start + trim_start;
    let end = start + raw.len();
    let (raw_parts, normalized_parts) = split_identifier_path_sql(raw, dialect_name)?;

    let masked = SqlParser::mask_sql_noise(text);
    let masked_selection = masked.get(start..end)?;
    let explicitly_quoted = raw_parts.iter().all(|part| {
        (part.starts_with('"') && part.ends_with('"'))
            || (part.starts_with('`') && part.ends_with('`'))
            || (part.starts_with('[') && part.ends_with(']'))
    });
    if masked_selection.trim_matches('.').trim().is_empty() && !explicitly_quoted {
        return None;
    }

    Some(IdentifierPathSelection {
        range: range_for_offsets(text, start, end),
        raw_parts,
        normalized_parts,
    })
}

pub(super) fn relation_name_matches(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
        || left
            .rsplit('.')
            .next()
            .zip(right.rsplit('.').next())
            .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
}

pub(super) fn column_qualification_sources(
    parser: &SqlParser,
    tree: &tree_sitter::Tree,
    text: &str,
    position: Position,
    schema: &Schema,
    dialect_name: &str,
) -> Vec<ColumnQualificationSource> {
    let aliases = SqlParser::relation_aliases_at_position(text, position);
    let byte_position = SqlParser::lsp_position_to_byte_position(text, position);
    let references = parser.extract_row_sources_at_position(tree, text, byte_position);
    let mut sources = Vec::new();

    for alias in &aliases {
        let Some(table) = schema
            .tables
            .iter()
            .find(|table| schema_table_matches(schema, &alias.relation, table))
        else {
            continue;
        };
        sources.push(ColumnQualificationSource {
            relation: alias.relation.clone(),
            qualifier_label: alias.name.clone(),
            qualifier_sql: alias.sql.clone(),
            columns: table
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect(),
            is_aliased: true,
        });
    }
    for reference in references {
        if aliases
            .iter()
            .any(|alias| relation_name_matches(&alias.relation, &reference))
        {
            continue;
        }
        let Some(table) = schema
            .tables
            .iter()
            .find(|table| schema_table_matches(schema, &reference, table))
        else {
            continue;
        };
        let qualifier_label = SqlParser::identifier_last_part(&reference);
        sources.push(ColumnQualificationSource {
            relation: reference,
            qualifier_sql: quote_completion_identifier(&qualifier_label, dialect_name),
            qualifier_label,
            columns: table
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect(),
            is_aliased: false,
        });
    }
    sources.sort_by(|left, right| {
        left.qualifier_label
            .cmp(&right.qualifier_label)
            .then_with(|| left.qualifier_sql.cmp(&right.qualifier_sql))
    });

    // An ordinary alias is case-insensitive, while a quoted alias remains a
    // distinct SQL identifier. Do not collapse `u` and `"U"`: doing so would
    // make a qualification action silently pick a different row source.
    let mut unique_sources = Vec::with_capacity(sources.len());
    for source in sources {
        if unique_sources
            .iter()
            .any(|existing| qualification_sources_are_equivalent(existing, &source))
        {
            continue;
        }
        unique_sources.push(source);
    }
    unique_sources
}

pub(super) fn sql_identifier_is_quoted(value: &str) -> bool {
    let value = value.trim();
    matches!(
        value.as_bytes(),
        [b'"', .., b'"'] | [b'`', .., b'`'] | [b'[', .., b']']
    )
}

pub(super) fn qualification_sources_are_equivalent(
    left: &ColumnQualificationSource,
    right: &ColumnQualificationSource,
) -> bool {
    if left.is_aliased || right.is_aliased {
        return left.is_aliased
            && right.is_aliased
            && if sql_identifier_is_quoted(&left.qualifier_sql)
                || sql_identifier_is_quoted(&right.qualifier_sql)
            {
                left.qualifier_sql == right.qualifier_sql
            } else {
                left.qualifier_label
                    .eq_ignore_ascii_case(&right.qualifier_label)
            };
    }

    left.qualifier_label
        .eq_ignore_ascii_case(&right.qualifier_label)
}

pub(super) fn source_matches_qualifier(
    source: &ColumnQualificationSource,
    raw_qualifier: &str,
    qualifier: &str,
) -> bool {
    if source.is_aliased {
        if sql_identifier_is_quoted(&source.qualifier_sql) {
            return source.qualifier_sql == raw_qualifier;
        }
        source.qualifier_label.eq_ignore_ascii_case(qualifier)
    } else {
        relation_name_matches(&source.relation, qualifier)
            || source.qualifier_label.eq_ignore_ascii_case(qualifier)
    }
}

pub(super) fn previous_sql_word(source: &str, offset: usize) -> Option<&str> {
    source[..offset.min(source.len())]
        .trim_end()
        .rsplit(|character: char| !character.is_alphanumeric() && character != '_')
        .find(|word| !word.is_empty())
}

pub(super) fn batch_qualify_identifier_action(
    text: &str,
    uri: &Url,
    request_range: Range,
    sources: &[ColumnQualificationSource],
    dialect_name: &str,
) -> Option<CodeAction> {
    const MAX_BATCH_QUALIFY_BYTES: usize = 256 * 1024;
    let start = position_to_byte_offset(text, request_range.start);
    let end = position_to_byte_offset(text, request_range.end);
    let (start, end) = (start.min(end), start.max(end).min(text.len()));
    if start >= end || end - start > MAX_BATCH_QUALIFY_BYTES {
        return None;
    }
    let masked = SqlParser::mask_sql_noise(text);
    let alias_names = sources
        .iter()
        .map(|source| source.qualifier_label.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let relation_names = sources
        .iter()
        .flat_map(|source| {
            [
                source.relation.to_ascii_lowercase(),
                source
                    .relation
                    .rsplit('.')
                    .next()
                    .unwrap_or(&source.relation)
                    .to_ascii_lowercase(),
            ]
        })
        .collect::<HashSet<_>>();
    let mut edits = Vec::new();
    let mut index = start;
    while index < end {
        let Some(first) = text[index..].chars().next() else {
            break;
        };
        if !SqlParser::is_identifier_char(first) && !matches!(first, '"' | '`' | '[') {
            index += first.len_utf8();
            continue;
        }
        let candidate_end = identifier_path_end(text, index).min(end);
        if candidate_end <= index {
            index += first.len_utf8();
            continue;
        }
        let raw = &text[index..candidate_end];
        let Some((raw_parts, normalized_parts)) = split_identifier_path_sql(raw, dialect_name)
        else {
            index += first.len_utf8();
            continue;
        };
        index = candidate_end;
        if normalized_parts.len() != 1 {
            continue;
        }
        let column = &normalized_parts[0];
        let normalized = column.to_ascii_lowercase();
        if Keywords::is_keyword(column)
            || alias_names.contains(&normalized)
            || relation_names.contains(&normalized)
        {
            continue;
        }
        let masked_candidate = &masked[candidate_end - raw.len()..candidate_end];
        let explicitly_quoted = raw_parts[0]
            .chars()
            .next()
            .is_some_and(|character| matches!(character, '"' | '`' | '['));
        if masked_candidate.trim().is_empty() && !explicitly_quoted {
            continue;
        }
        if previous_sql_word(&masked, candidate_end - raw.len()).is_some_and(|word| {
            matches!(
                word.to_ascii_uppercase().as_str(),
                "AS" | "FROM" | "JOIN" | "APPLY" | "UPDATE" | "INTO" | "TABLE" | "VIEW"
            )
        }) {
            continue;
        }
        if text[candidate_end..]
            .chars()
            .find(|character| !character.is_whitespace())
            == Some('(')
        {
            continue;
        }
        let matching_sources = sources
            .iter()
            .filter(|source| {
                source
                    .columns
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(column))
            })
            .collect::<Vec<_>>();
        if matching_sources.len() != 1 {
            continue;
        }
        edits.push(TextEdit {
            range: range_for_offsets(text, candidate_end - raw.len(), candidate_end),
            new_text: format!("{}.{}", matching_sources[0].qualifier_sql, raw_parts[0]),
        });
    }
    if edits.is_empty() {
        return None;
    }
    let count = edits.len();
    Some(CodeAction {
        title: format!(
            "Qualify {count} selected column{}",
            if count == 1 { "" } else { "s" }
        ),
        kind: Some(CodeActionKind::REFACTOR_REWRITE),
        edit: Some(WorkspaceEdit {
            changes: Some(HashMap::from([(uri.clone(), edits)])),
            document_changes: None,
            change_annotations: None,
        }),
        is_preferred: Some(true),
        ..Default::default()
    })
}

pub(super) fn qualify_identifier_actions(
    text: &str,
    uri: &Url,
    request_range: Range,
    schema: Option<&Schema>,
    dialect_name: &str,
) -> Vec<CodeAction> {
    let Some(schema) = schema else {
        return Vec::new();
    };
    let request_start = position_to_byte_offset(text, request_range.start);
    let request_end = position_to_byte_offset(text, request_range.end);
    if request_start != request_end {
        let mut parser = SqlParser::new();
        let parsed = parser.parse(text);
        let Some(tree) = parsed.tree.as_ref() else {
            return Vec::new();
        };
        let sources = column_qualification_sources(
            &parser,
            tree,
            text,
            request_range.start,
            schema,
            dialect_name,
        );
        return batch_qualify_identifier_action(text, uri, request_range, &sources, dialect_name)
            .into_iter()
            .collect();
    }
    let Some(selection) = identifier_path_selection(text, request_range, dialect_name) else {
        return Vec::new();
    };
    let Some(column) = selection.normalized_parts.last() else {
        return Vec::new();
    };
    if Keywords::is_keyword(column) {
        return Vec::new();
    }
    let before = text[..position_to_byte_offset(text, selection.range.start)].trim_end();
    if before
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .next_back()
        .is_some_and(|word| word.eq_ignore_ascii_case("AS"))
    {
        return Vec::new();
    }

    let mut parser = SqlParser::new();
    let parsed = parser.parse(text);
    let Some(tree) = parsed.tree.as_ref() else {
        return Vec::new();
    };
    let sources = column_qualification_sources(
        &parser,
        tree,
        text,
        request_range.start,
        schema,
        dialect_name,
    );
    if sources.is_empty()
        || sources
            .iter()
            .any(|source| source.qualifier_label.eq_ignore_ascii_case(column))
    {
        return Vec::new();
    }
    let matching_sources = sources
        .iter()
        .filter(|source| {
            source
                .columns
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(column))
        })
        .collect::<Vec<_>>();
    let column_sql = selection.raw_parts.last().cloned().unwrap_or_default();

    if selection.normalized_parts.len() > 1 {
        let qualifier =
            selection.normalized_parts[..selection.normalized_parts.len() - 1].join(".");
        let raw_qualifier = selection.raw_parts[..selection.raw_parts.len() - 1].join(".");
        if matching_sources.len() != 1
            || !source_matches_qualifier(matching_sources[0], &raw_qualifier, &qualifier)
        {
            return Vec::new();
        }
        return vec![CodeAction {
            title: format!("Remove qualifier from {}", column_sql),
            kind: Some(CodeActionKind::REFACTOR_REWRITE),
            edit: Some(single_document_edit(
                uri.clone(),
                selection.range,
                column_sql,
            )),
            is_preferred: Some(true),
            ..Default::default()
        }];
    }

    matching_sources
        .into_iter()
        .map(|source| {
            let replacement = format!("{}.{}", source.qualifier_sql, column_sql);
            CodeAction {
                title: format!("Qualify column as {replacement}"),
                kind: Some(CodeActionKind::REFACTOR_REWRITE),
                edit: Some(single_document_edit(
                    uri.clone(),
                    selection.range,
                    replacement,
                )),
                is_preferred: Some(sources.len() == 1),
                ..Default::default()
            }
        })
        .collect()
}

pub(super) fn add_mutation_safety_guard_action(
    text: &str,
    uri: &Url,
    diagnostic: &Diagnostic,
) -> Option<CodeAction> {
    let start = position_to_byte_offset(text, diagnostic.range.start);
    let end = position_to_byte_offset(text, diagnostic.range.end).min(text.len());
    let statement = text.get(start..end)?;
    let trailing = statement.trim_end();
    let insert_offset = if trailing.ends_with(';') {
        start + trailing.len() - 1
    } else {
        start + trailing.len()
    };
    let insert_range = range_for_offsets(text, insert_offset, insert_offset);
    Some(CodeAction {
        title: "Add non-matching WHERE safety guard".to_string(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic.clone()]),
        edit: Some(single_document_edit(
            uri.clone(),
            insert_range,
            " WHERE 1 = 0 /* replace safety guard before executing */".to_string(),
        )),
        is_preferred: Some(true),
        ..Default::default()
    })
}

pub(super) fn expand_select_star_action(
    text: &str,
    uri: &Url,
    request_range: Range,
    schema: Option<Schema>,
    dialect_name: &str,
) -> Option<CodeAction> {
    let schema = schema?;
    let selection_start = position_to_byte_offset(text, request_range.start);
    let selection_end = position_to_byte_offset(text, request_range.end);
    let star = text.match_indices('*').find_map(|(offset, _)| {
        let selected = selection_start <= offset && offset <= selection_end.max(selection_start);
        let nearby = offset.abs_diff(selection_start) <= 2;
        (selected || nearby).then_some(offset)
    })?;
    let prefix = text[..star].to_ascii_uppercase();
    let select_start = prefix.rfind("SELECT")?;
    let mut projection_prefix = text[select_start + "SELECT".len()..star].trim();
    if projection_prefix
        .get(.."DISTINCT".len())
        .is_some_and(|value| value.eq_ignore_ascii_case("DISTINCT"))
    {
        projection_prefix = projection_prefix["DISTINCT".len()..].trim();
    }
    let qualifier_sql = if projection_prefix.is_empty() {
        None
    } else if projection_prefix.ends_with('.') {
        let qualifier = projection_prefix.trim_end_matches('.').trim();
        (!qualifier.is_empty()).then_some(qualifier.to_string())
    } else {
        return None;
    };
    let qualifier = qualifier_sql
        .as_deref()
        .map(SqlParser::normalize_identifier);
    let statement_start = text[..star].rfind(';').map_or(0, |offset| offset + 1);
    let statement_end = text[star..]
        .find(';')
        .map_or(text.len(), |offset| star + offset + 1);
    let statement = &text[statement_start..statement_end];
    let mut parser = SqlParser::new();
    let parse_result = parser.parse(statement);
    let tree = parse_result.tree.as_ref()?;
    let references = parser.extract_referenced_tables(tree, statement);
    let aliases = parser.extract_aliases(tree, statement);
    let mut sources = Vec::<(&Table, String)>::new();

    if let Some(qualifier) = qualifier.as_deref() {
        let target = aliases
            .get(qualifier)
            .map(String::as_str)
            .unwrap_or(qualifier);
        let table = schema
            .tables
            .iter()
            .find(|table| schema_table_matches(&schema, target, table))?;
        sources.push((table, qualifier_sql.clone()?));
    } else {
        if references.is_empty() {
            return None;
        }
        let mut used_aliases = HashSet::new();
        for reference in references {
            let table = schema
                .tables
                .iter()
                .find(|table| schema_table_matches(&schema, &reference, table))?;
            let mut matching_aliases = aliases
                .iter()
                .filter(|(alias, target)| {
                    !used_aliases.contains(*alias) && schema_table_matches(&schema, target, table)
                })
                .map(|(alias, _)| alias.clone())
                .collect::<Vec<_>>();
            matching_aliases.sort();
            let source_qualifier = matching_aliases
                .into_iter()
                .next()
                .unwrap_or_else(|| SqlParser::identifier_last_part(&reference));
            used_aliases.insert(source_qualifier.clone());
            sources.push((table, source_qualifier));
        }
    }
    if sources.is_empty()
        || sources
            .iter()
            .any(|(table, _)| table.columns.iter().all(|column| column.hidden))
    {
        return None;
    }

    let qualified_expansion = qualifier.is_none() && sources.len() > 1;
    let mut replacement_columns = Vec::new();
    for (table, source_qualifier) in &sources {
        for (index, column) in table
            .columns
            .iter()
            .filter(|column| !column.hidden)
            .enumerate()
        {
            let column_name = if qualifier.is_some() {
                // The original `alias.` remains before the replacement range.
                if index == 0 {
                    quote_completion_identifier(&column.name, dialect_name)
                } else {
                    format!(
                        "{}.{}",
                        quote_completion_identifier(source_qualifier, dialect_name),
                        quote_completion_identifier(&column.name, dialect_name)
                    )
                }
            } else if qualified_expansion {
                format!(
                    "{}.{}",
                    quote_completion_identifier(source_qualifier, dialect_name),
                    quote_completion_identifier(&column.name, dialect_name)
                )
            } else {
                quote_completion_identifier(&column.name, dialect_name)
            };
            replacement_columns.push(column_name);
        }
    }
    let replacement = replacement_columns.join(", ");
    let range = range_for_offsets(text, star, star + 1);
    Some(CodeAction {
        title: format!("Expand * to {} columns", replacement_columns.len()),
        kind: Some(CodeActionKind::REFACTOR_REWRITE),
        edit: Some(single_document_edit(uri.clone(), range, replacement)),
        is_preferred: Some(true),
        ..Default::default()
    })
}
