use crate::parser::SqlParser;
use crate::schema::{Function, Schema, Table};
use std::collections::HashSet;
use tower_lsp::lsp_types::Position;

pub(crate) fn table_matches(schema: &Schema, table: &Table, reference: &str) -> bool {
    SqlParser::table_name_matches_with_catalog(
        reference,
        schema.catalog.as_deref(),
        &schema.database,
        &table.name,
    )
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
    SqlParser::column_qualifier_before_position(sql, position).or_else(|| {
        let node = parser.get_node_at_position(tree, position)?;
        parser.get_table_name_for_column(node, sql)
    })
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
) -> Vec<String> {
    let referenced_tables = parser.extract_referenced_tables_at_position(tree, sql, position);
    let aliases = SqlParser::relation_aliases_at_position(sql, position);
    let mut seen = HashSet::new();
    referenced_tables
        .iter()
        .map(|table| {
            SqlParser::resolve_relation_alias(&aliases, table)
                .map(|alias| alias.relation.clone())
                .unwrap_or_else(|| table.clone())
        })
        .filter(|table| seen.insert(table.to_ascii_lowercase()))
        .collect()
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
