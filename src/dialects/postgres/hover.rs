use super::PostgresDialect;
use crate::{parser::SqlParser, schema::Schema};
use tower_lsp::lsp_types::{Hover, MarkedString, Position};

pub(super) fn hover(
    dialect: &PostgresDialect,
    sql: &str,
    position: Position,
    schema: Option<&Schema>,
) -> Option<Hover> {
    let position = SqlParser::lsp_position_to_byte_position(sql, position);
    let mut parser = dialect.parser.lock().unwrap();
    let parse_result = parser.parse(sql);

    if let (Some(schema), Some(tree)) = (schema, &parse_result.tree) {
        if let Some(node) = parser.get_node_at_position(tree, position) {
            let node_text = parser.node_text(node, sql);
            let node_kind = node.kind();

            if crate::token::Keywords::is_keyword(&node_text)
                || crate::token::Operators::is_operator(&node_text)
                || crate::token::Delimiters::is_delimiter(&node_text)
            {
                return None;
            }

            let relation = crate::dialects::common::relation_reference_node(node);
            let is_table = relation.kind() == "object_reference"
                && relation
                    .parent()
                    .is_some_and(|parent| parent.kind() == "relation");

            if is_table {
                let table_ref = parser.node_text(relation, sql);
                if let Some(table) = PostgresDialect::find_table_by_reference(schema, &table_ref) {
                    return Some(Hover {
                        contents: tower_lsp::lsp_types::HoverContents::Scalar(
                            MarkedString::String(format!(
                                "{} Table: {}.{}\n{}",
                                dialect.display_name(),
                                schema.database,
                                table.name,
                                table
                                    .documentation()
                                    .unwrap_or_else(|| "No description".to_string())
                            )),
                        ),
                        range: Some(parser.node_range(node)),
                    });
                }
            }

            let is_column = node_kind == "column_name"
                || node_kind == "column_reference"
                || node_kind == "column_identifier"
                || (node_kind == "identifier" && parser.is_in_column_context(node, sql));

            if is_column {
                if let Some((table, column)) = crate::dialects::common::column_at_position(
                    &parser, tree, node, sql, position, schema,
                ) {
                    return Some(Hover {
                        contents: tower_lsp::lsp_types::HoverContents::Scalar(
                            MarkedString::String(format!(
                                "{} Column: {}{}\n{}",
                                dialect.display_name(),
                                if table.is_empty() {
                                    String::new()
                                } else {
                                    format!("{table}.")
                                },
                                column.name,
                                column
                                    .documentation()
                                    .unwrap_or_else(|| "No description".to_string())
                            )),
                        ),
                        range: Some(parser.node_range(node)),
                    });
                }
            }

            if PostgresDialect::is_function_reference(node, sql) {
                if let Some(function) =
                    PostgresDialect::find_function_by_reference(schema, &node_text)
                {
                    return Some(Hover {
                        contents: tower_lsp::lsp_types::HoverContents::Markup(
                            tower_lsp::lsp_types::MarkupContent {
                                kind: tower_lsp::lsp_types::MarkupKind::Markdown,
                                value: function.markdown_documentation(),
                            },
                        ),
                        range: Some(parser.node_range(node)),
                    });
                }
            }
        }
    }

    None
}
