use super::MysqlDialect;
use crate::{parser::SqlParser, schema::Schema};
use tower_lsp::lsp_types::{Hover, Position};

pub(super) fn hover(
    dialect: &MysqlDialect,
    sql: &str,
    position: Position,
    schema: Option<&Schema>,
) -> Option<Hover> {
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
                let is_table = node_kind == "table_name"
                    || node_kind == "table_reference"
                    || node_kind == "table_identifier"
                    || (node_kind == "identifier" && parser.is_in_from_context(node, sql));

                if is_table {
                    let table_ref = SqlParser::normalize_identifier(&node_text);
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
                    // 尝试获取表名（如果是 table.column 格式）
                    let table_name = parser.get_table_name_for_column(node, sql);

                    for table in &schema.tables {
                        // 如果有明确的表名，只在该表中查找
                        if let Some(ref tname) = table_name {
                            if !MysqlDialect::table_matches(schema, table, tname) {
                                continue;
                            }
                        }

                        let column_name = SqlParser::identifier_last_part(&node_text);
                        if let Some(column) = table.columns.iter().find(|c| c.name == column_name) {
                            let mut info =
                                format!("**Column**: `{}.{}`\n\n", table.name, column.name);
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
