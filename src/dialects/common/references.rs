use crate::parser::SqlParser;
use crate::schema::{Function, Schema, Table};
use std::collections::HashSet;
use tower_lsp::lsp_types::Position;

pub(crate) fn table_matches(schema: &Schema, table: &Table, reference: &str) -> bool {
    crate::relation_bindings::table_matches(schema, table, reference)
}

pub(crate) fn table_is_referenced(schema: &Schema, table: &Table, references: &[String]) -> bool {
    references.is_empty()
        || references
            .iter()
            .any(|reference| table_matches(schema, table, reference))
}

pub(crate) fn find_table_by_reference<'a>(
    schema: &'a Schema,
    reference: &str,
) -> Option<&'a Table> {
    schema
        .tables
        .iter()
        .find(|table| table_matches(schema, table, reference))
}

pub(crate) fn table_column_reference_at_position(
    parser: &SqlParser,
    tree: &tree_sitter::Tree,
    sql: &str,
    position: Position,
) -> Option<String> {
    parser
        .column_qualifier_at_position(sql, position)
        .or_else(|| {
            let node = parser.get_node_at_position(tree, position)?;
            parser.get_table_name_for_column(node, sql)
        })
}

pub(crate) fn row_reference<'a>(
    schema: &Schema,
    aliases: &'a [crate::parser::RelationAlias],
    reference: &'a str,
) -> &'a str {
    let Some(alias) = SqlParser::resolve_relation_alias(aliases, reference) else {
        return reference;
    };
    if schema.tables.iter().any(|table| {
        matches!(
            table.object_type.as_deref(),
            Some("TABLE FUNCTION" | "CORRELATED TABLE" | "DERIVED TABLE")
        ) && table.name == alias.name
    }) {
        &alias.sql
    } else {
        &alias.relation
    }
}

#[cfg(test)]
pub(crate) fn referenced_table_names(
    parser: &SqlParser,
    tree: &tree_sitter::Tree,
    sql: &str,
) -> Vec<String> {
    let referenced_tables = parser.extract_referenced_tables(tree, sql);
    let aliases = parser.extract_aliases(tree, sql);
    let mut seen = HashSet::new();
    referenced_tables
        .iter()
        .map(|table| aliases.get(table).unwrap_or(table).clone())
        .filter(|table| seen.insert(table.to_ascii_lowercase()))
        .collect()
}

pub(crate) fn referenced_table_names_at_position(
    parser: &SqlParser,
    tree: &tree_sitter::Tree,
    sql: &str,
    position: Position,
    schema: &Schema,
) -> Vec<String> {
    let mut seen = HashSet::new();
    row_references_at_position(parser, tree, sql, position, schema)
        .into_iter()
        .filter(|table| seen.insert(table.to_ascii_lowercase()))
        .collect()
}

/// One entry per row-source occurrence, including two aliases of one table.
pub(crate) fn row_references_at_position(
    parser: &SqlParser,
    tree: &tree_sitter::Tree,
    sql: &str,
    position: Position,
    schema: &Schema,
) -> Vec<String> {
    if parser.uses_joined_scope(tree, sql, position) {
        if let Some(scope) = parser.relation_scope_for_member(tree, sql, position) {
            if let Some(occurrences) = &scope.occurrences {
                return occurrences
                    .iter()
                    .map(|(reference, alias)| {
                        alias.as_ref().map_or_else(
                            || reference.clone(),
                            |a| row_reference(schema, &scope.aliases, &a.sql).to_owned(),
                        )
                    })
                    .collect();
            }
        }
    }
    let referenced_tables = parser.extract_row_sources_at_position(tree, sql, position);
    let aliases = if parser.uses_joined_scope(tree, sql, position) {
        parser
            .relation_scope_for_member(tree, sql, position)
            .map(|s| s.aliases)
            .unwrap_or_default()
    } else {
        SqlParser::relation_aliases_at_position(sql, position)
    };
    let mut result = Vec::new();
    for reference in &referenced_tables {
        let bindings: Vec<_> = aliases
            .iter()
            .filter(|alias| alias.relation.eq_ignore_ascii_case(reference))
            .collect();
        if bindings.is_empty() {
            result.push(reference.clone());
        } else {
            for alias in bindings {
                result.push(row_reference(schema, &aliases, &alias.sql).to_string());
            }
        }
    }
    for alias in &aliases {
        if schema.tables.iter().any(|table| {
            table.object_type.as_deref() == Some("DERIVED TABLE") && table.name == alias.name
        }) {
            result.push(alias.sql.clone());
        }
    }
    result
}

pub(crate) fn function_name_from_reference(reference: &str) -> String {
    let before_args = reference.split('(').next().unwrap_or(reference);
    SqlParser::identifier_last_part(before_args)
}

pub(crate) fn find_function_by_reference<'a>(
    schema: &'a Schema,
    reference: &str,
) -> Option<&'a Function> {
    let function_name = function_name_from_reference(reference);
    schema.functions.iter().find(|function| {
        function.name == function_name || function.name.eq_ignore_ascii_case(&function_name)
    })
}

pub(crate) fn is_function_reference(node: tree_sitter::Node, sql: &str) -> bool {
    let node_kind = node.kind();
    if node_kind == "function_name" || node_kind.contains("function") {
        return true;
    }

    let end_byte = node.end_byte().min(sql.len());
    sql[end_byte..].trim_start().starts_with('(')
}

/// Retain a schema/catalog qualifier when the cursor selects a relation leaf.
pub(crate) fn relation_reference_node(mut node: tree_sitter::Node<'_>) -> tree_sitter::Node<'_> {
    while let Some(parent) = node.parent().filter(|parent| {
        matches!(
            parent.kind(),
            "object_reference" | "table_reference" | "table_name" | "table_identifier"
        )
    }) {
        node = parent;
    }
    node
}
