//! Cursor- and source-bound hover for SQL providers sharing the generic grammar.
use super::references::{
    find_function_by_reference, find_table_by_reference, is_function_reference,
};
use super::scope_columns::column_at_position;
use crate::{parser::SqlParser, schema::Schema};
use tower_lsp::lsp_types::{Hover, HoverContents, MarkedString, Position};

pub(crate) fn schema_hover(
    parser: &mut SqlParser,
    sql: &str,
    position: Position,
    schema: &Schema,
    label: &str,
) -> Option<Hover> {
    let position = SqlParser::lsp_position_to_byte_position(sql, position);
    let tree = parser.parse_syntax_tree(sql)?;
    let node = parser.get_node_at_position(&tree, position)?;
    let raw = parser.node_text(node, sql);
    let relation = super::references::relation_reference_node(node);
    if relation
        .parent()
        .is_some_and(|parent| parent.kind() == "relation")
        && relation.kind() == "object_reference"
    {
        let reference = parser.node_text(relation, sql);
        if let Some(table) = find_table_by_reference(schema, &reference) {
            return Some(Hover {
                contents: HoverContents::Scalar(MarkedString::String(format!(
                    "{label} Table: {}.{}\n{}",
                    schema.database,
                    table.name,
                    table
                        .documentation()
                        .unwrap_or_else(|| "No description".into())
                ))),
                range: Some(parser.node_range(relation)),
            });
        }
    }
    if parser.is_in_column_context(node, sql)
        && matches!(
            node.kind(),
            "identifier" | "field" | "column_reference" | "column_name" | "column_identifier"
        )
    {
        let (table, column) = column_at_position(parser, &tree, node, sql, position, schema)?;
        return Some(Hover {
            contents: HoverContents::Scalar(MarkedString::String(format!(
                "{label} Column: {}{}\n{}",
                if table.is_empty() {
                    String::new()
                } else {
                    format!("{table}.")
                },
                column.name,
                column
                    .documentation()
                    .unwrap_or_else(|| "No description".into())
            ))),
            range: Some(parser.node_range(node)),
        });
    }
    if is_function_reference(node, sql) {
        if let Some(function) = find_function_by_reference(schema, &raw) {
            return Some(Hover {
                contents: HoverContents::Scalar(MarkedString::String(
                    function.markdown_documentation(),
                )),
                range: Some(parser.node_range(node)),
            });
        }
    }
    None
}
