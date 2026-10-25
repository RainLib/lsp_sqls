use super::MysqlDialect;
use crate::{parser::SqlParser, schema::Schema};
use tower_lsp::lsp_types::{Hover, Position};

pub(super) fn hover(
    dialect: &MysqlDialect,
    sql: &str,
    position: Position,
    schema: Option<&Schema>,
) -> Option<Hover> {
    let position = SqlParser::lsp_position_to_byte_position(sql, position);
    let mut parser = dialect.parser.lock().unwrap();
    let parse_result = parser.parse(sql);

    // 获取光标位置的节点
    if let Some(tree) = &parse_result.tree {
        if let Some(node) = parser.get_node_at_position(tree, position) {
            let node_text = parser.node_text(node, sql);
            let node_kind = node.kind();
            let node_range = parser.node_range(node);

            // 过滤关键字、操作符、分隔符
            if crate::token::Keywords::is_keyword(&node_text)
                || crate::token::Operators::is_operator(&node_text)
                || crate::token::Delimiters::is_delimiter(&node_text)
            {
                return None;
            }

            if let Some(schema) = schema {
                // 检查是否是表名
                let relation = crate::dialects::common::relation_reference_node(node);
                let is_table = relation.kind() == "object_reference"
                    && relation
                        .parent()
                        .is_some_and(|parent| parent.kind() == "relation");

                if is_table {
                    let table_ref = parser.node_text(relation, sql);
                    if let Some(table) = MysqlDialect::find_table_by_reference(schema, &table_ref) {
                        let mut info = format!("**Table**: `{}`\n\n", table.name);
                        if let Some(comment) = &table.comment {
                            info.push_str(&format!("{}\n\n", comment));
                        }
                        info.push_str(&format!("**Columns** ({})\n", table.columns.len()));
                        for (idx, col) in table.columns.iter().take(10).enumerate() {
                            info.push_str(&format!(
                                "- `{}`: {} {}\n",
                                col.name,
                                col.data_type,
                                if col.nullable { "" } else { "NOT NULL" }
                            ));
                            if idx == 9 && table.columns.len() > 10 {
                                info.push_str(&format!(
                                    "- ... and {} more\n",
                                    table.columns.len() - 10
                                ));
                                break;
                            }
                        }

                        return Some(Hover {
                            contents: tower_lsp::lsp_types::HoverContents::Markup(
                                tower_lsp::lsp_types::MarkupContent {
                                    kind: tower_lsp::lsp_types::MarkupKind::Markdown,
                                    value: info,
                                },
                            ),
                            range: Some(node_range),
                        });
                    }
                }

                // 检查是否是列名
                let is_column = node_kind == "column_name"
                    || node_kind == "column_reference"
                    || node_kind == "column_identifier"
                    || (node_kind == "identifier" && parser.is_in_column_context(node, sql));

                if is_column {
                    if let Some((table, column)) = crate::dialects::common::column_at_position(
                        &parser, tree, node, sql, position, schema,
                    ) {
                        let prefix = if table.is_empty() {
                            String::new()
                        } else {
                            format!("{table}.")
                        };
                        let mut info = format!("**Column**: `{prefix}{}`\n\n", column.name);
                        info.push_str(&format!("**Type**: `{}`\n", column.data_type));
                        info.push_str(&format!(
                            "**Nullable**: {}\n",
                            if column.nullable { "Yes" } else { "No" }
                        ));
                        if let Some(comment) = &column.comment {
                            info.push_str(&format!("\n{}\n", comment));
                        }

                        return Some(Hover {
                            contents: tower_lsp::lsp_types::HoverContents::Markup(
                                tower_lsp::lsp_types::MarkupContent {
                                    kind: tower_lsp::lsp_types::MarkupKind::Markdown,
                                    value: info,
                                },
                            ),
                            range: Some(node_range),
                        });
                    }
                }

                // 检查是否是函数名
                if MysqlDialect::is_function_reference(node, sql) {
                    if let Some(func) = MysqlDialect::find_function_by_reference(schema, &node_text)
                    {
                        return Some(Hover {
                            contents: tower_lsp::lsp_types::HoverContents::Markup(
                                tower_lsp::lsp_types::MarkupContent {
                                    kind: tower_lsp::lsp_types::MarkupKind::Markdown,
                                    value: func.markdown_documentation(),
                                },
                            ),
                            range: Some(node_range),
                        });
                    }
                }
            }
        }
    }

    None
}
