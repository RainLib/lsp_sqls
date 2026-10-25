//! Join actions for the SQL language server.
use super::completion_context::position_to_byte_offset;
use super::completion_identifiers::quote_completion_identifier;
use super::diagnostics::InspectionContext;
use super::schema_resolution::schema_table_matches;
use super::signature_help::single_document_edit;
use crate::parser::SqlParser;
use crate::schema::Constraint;
use crate::schema::Schema;
use crate::schema::Table;
use std::collections::HashMap;
use std::collections::HashSet;
use tower_lsp::lsp_types::*;
pub(super) fn foreign_key_targets_table(
    schema: &Schema,
    constraint: &Constraint,
    target: &Table,
) -> bool {
    if !constraint
        .constraint_type
        .to_ascii_uppercase()
        .contains("FOREIGN")
    {
        return false;
    }
    let Some(referenced_table) = constraint.referenced_table.as_deref() else {
        return false;
    };
    let reference = constraint
        .referenced_schema
        .as_deref()
        .filter(|schema| !schema.trim().is_empty())
        .map(|referenced_schema| format!("{referenced_schema}.{referenced_table}"))
        .unwrap_or_else(|| referenced_table.to_string());
    schema_table_matches(schema, &reference, target)
}

pub(super) fn join_reference_qualifier(
    schema: &Schema,
    table: &Table,
    reference: &str,
    aliases: &HashMap<String, String>,
    dialect_name: &str,
) -> Option<String> {
    let mut matching_aliases = aliases
        .iter()
        .filter(|(_, target)| schema_table_matches(schema, target, table))
        .map(|(alias, _)| alias.as_str());
    let alias = matching_aliases.next();
    if alias.is_some() && matching_aliases.next().is_some() {
        // Multiple aliases for the same relation form a self-join-like shape;
        // the alias map does not prove which occurrence owns this constraint.
        return None;
    }
    Some(quote_completion_identifier(
        alias
            .map(str::to_string)
            .unwrap_or_else(|| SqlParser::identifier_last_part(reference))
            .as_str(),
        dialect_name,
    ))
}

pub(super) fn join_predicate(
    left_qualifier: &str,
    left_columns: &[String],
    right_qualifier: &str,
    right_columns: &[String],
    dialect_name: &str,
) -> Option<String> {
    if left_columns.is_empty() || left_columns.len() != right_columns.len() {
        return None;
    }
    Some(
        left_columns
            .iter()
            .zip(right_columns)
            .map(|(left, right)| {
                format!(
                    "{left_qualifier}.{} = {right_qualifier}.{}",
                    quote_completion_identifier(left, dialect_name),
                    quote_completion_identifier(right, dialect_name),
                )
            })
            .collect::<Vec<_>>()
            .join(" AND "),
    )
}

pub(super) fn add_join_condition_actions(
    text: &str,
    uri: &Url,
    diagnostic: &Diagnostic,
    schema: Option<&Schema>,
    dialect_name: &str,
) -> Vec<CodeAction> {
    const MAX_INFERRED_JOIN_ACTIONS: usize = 16;

    let join_start = position_to_byte_offset(text, diagnostic.range.start).min(text.len());
    let context = InspectionContext::new(text, dialect_name);
    let Some(span) = context.statement_at(join_start) else {
        return Vec::new();
    };
    let Some(join) = context.joins(&span).find(|join| join.keyword == join_start) else {
        return Vec::new();
    };
    let Some(insert_offset) = join.insert else {
        return Vec::new();
    };
    let insert_range = context.range(insert_offset, insert_offset);
    let fallback = || CodeAction {
        title: "Add non-matching JOIN safety guard".to_string(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic.clone()]),
        edit: Some(single_document_edit(
            uri.clone(),
            insert_range,
            " ON 1 = 0 /* replace join condition before executing */".to_string(),
        )),
        is_preferred: Some(false),
        ..Default::default()
    };

    let Some(schema) = schema else {
        return vec![fallback()];
    };
    let join_prefix = &text[join.scope_start..insert_offset];
    let mut parser = SqlParser::new_with_placeholder_dialect(
        crate::sql_lexical::lexical_rules_for(dialect_name).placeholder,
    );
    let Some(tree) = parser.parse(join_prefix).tree else {
        return vec![fallback()];
    };
    let position = crate::position::byte_position_at_end(join_prefix);
    let references = parser.extract_referenced_tables_at_position(&tree, join_prefix, position);
    let Some(candidate_reference) = references.last() else {
        return vec![fallback()];
    };
    if references.len() < 2 {
        return vec![fallback()];
    }
    let Some(candidate) = schema
        .tables
        .iter()
        .find(|table| schema_table_matches(schema, candidate_reference, table))
    else {
        return vec![fallback()];
    };
    let aliases = parser.extract_aliases_at_position(&tree, join_prefix, position);
    let Some(candidate_qualifier) = join_reference_qualifier(
        schema,
        candidate,
        candidate_reference,
        &aliases,
        dialect_name,
    ) else {
        return vec![fallback()];
    };

    let mut inferred = Vec::<(String, String)>::new();
    let mut seen = HashSet::new();
    for existing_reference in &references[..references.len() - 1] {
        let Some(existing) = schema
            .tables
            .iter()
            .find(|table| schema_table_matches(schema, existing_reference, table))
        else {
            continue;
        };
        if std::ptr::eq(candidate, existing) {
            continue;
        }
        let Some(existing_qualifier) =
            join_reference_qualifier(schema, existing, existing_reference, &aliases, dialect_name)
        else {
            continue;
        };

        for constraint in &candidate.constraints {
            if !foreign_key_targets_table(schema, constraint, existing) {
                continue;
            }
            if let Some(predicate) = join_predicate(
                &candidate_qualifier,
                &constraint.columns,
                &existing_qualifier,
                &constraint.referenced_columns,
                dialect_name,
            ) {
                if seen.insert(predicate.to_ascii_lowercase()) {
                    inferred.push((constraint.name.clone(), predicate));
                }
            }
        }
        for constraint in &existing.constraints {
            if !foreign_key_targets_table(schema, constraint, candidate) {
                continue;
            }
            if let Some(predicate) = join_predicate(
                &existing_qualifier,
                &constraint.columns,
                &candidate_qualifier,
                &constraint.referenced_columns,
                dialect_name,
            ) {
                if seen.insert(predicate.to_ascii_lowercase()) {
                    inferred.push((constraint.name.clone(), predicate));
                }
            }
        }
        if inferred.len() >= MAX_INFERRED_JOIN_ACTIONS {
            break;
        }
    }
    inferred.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    inferred.truncate(MAX_INFERRED_JOIN_ACTIONS);
    let unique = inferred.len() == 1;
    let mut actions = inferred
        .into_iter()
        .map(|(constraint_name, predicate)| CodeAction {
            title: if constraint_name.trim().is_empty() {
                "Add JOIN condition from foreign-key metadata".to_string()
            } else {
                format!("Add JOIN condition via {constraint_name}")
            },
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(single_document_edit(
                uri.clone(),
                insert_range,
                format!(" ON {predicate}"),
            )),
            is_preferred: Some(unique),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    actions.push(fallback());
    actions
}
