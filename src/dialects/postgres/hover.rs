use super::PostgresDialect;
use crate::{parser::SqlParser, schema::Schema};
use tower_lsp::lsp_types::{Hover, MarkedString, Position};

pub(super) fn hover(
    dialect: &PostgresDialect,
    sql: &str,
    position: Position,
    schema: Option<&Schema>,
) -> Option<Hover> {
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

            let is_table = node_kind == "table_name"
                || node_kind == "table_reference"
                || node_kind == "table_identifier"
                || (node_kind == "identifier" && parser.is_in_from_context(node, sql));

            if is_table {
                let table_ref = SqlParser::normalize_identifier(&node_text);
                if let Some(table) = PostgresDialect::find_table_by_reference(schema, &table_ref) {
                    return Some(Hover {
                        contents: tower_lsp::lsp_types::HoverContents::Scalar(
                            MarkedString::String(format!(
                                "PostgreSQL Table: {}.{}\n{}",
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
                let table_name = parser.get_table_name_for_column(node, sql);
                let column_name = SqlParser::identifier_last_part(&node_text);

                for table in &schema.tables {
                    if let Some(ref table_ref) = table_name {
                        if !PostgresDialect::table_matches(schema, table, table_ref) {
                            continue;
                        }
                    }

                    if let Some(column) = table
                        .columns
                        .iter()
                        .find(|column| column.name == column_name)
                    {
                        return Some(Hover {
                            contents: tower_lsp::lsp_types::HoverContents::Scalar(
                                MarkedString::String(format!(
                                    "PostgreSQL Column: {}.{}\n{}",
                                    table.name,
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
