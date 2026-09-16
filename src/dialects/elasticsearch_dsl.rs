mod completion;
mod context;
mod http;

use crate::dialect::Dialect;
use crate::dialects::common;
use crate::parser::dsl::DslParser;
use crate::schema::Schema;
use async_trait::async_trait;
use tower_lsp::lsp_types::{CompletionItem, Diagnostic, Hover, Location, MarkedString, Position};

/// Elasticsearch DSL (Domain Specific Language) dialect.
pub struct ElasticsearchDslDialect {
    dsl_parser: std::sync::Mutex<DslParser>,
}

impl Default for ElasticsearchDslDialect {
    fn default() -> Self {
        Self::new()
    }
}

impl ElasticsearchDslDialect {
    pub fn new() -> Self {
        Self {
            dsl_parser: std::sync::Mutex::new(DslParser::new()),
        }
    }
}

/// Recursively collect field-key references in the active DSL document.
fn find_field_references_recursive(
    node: tree_sitter::Node,
    source: &str,
    field_name: &str,
    uri: &tower_lsp::lsp_types::Url,
    locations: &mut Vec<Location>,
    parser: &crate::parser::dsl::DslParser,
) {
    if node.kind() == "pair" {
        if let Some(key_node) = node.child(0) {
            if let Ok(key_text) = key_node.utf8_text(source.as_bytes()) {
                let key = key_text.trim_matches('"').trim_matches('\'');
                if key == field_name {
                    locations.push(Location {
                        uri: uri.clone(),
                        range: parser.node_range(key_node, source),
                    });
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        find_field_references_recursive(child, source, field_name, uri, locations, parser);
    }
}

#[async_trait]
impl Dialect for ElasticsearchDslDialect {
    fn name(&self) -> &str {
        "elasticsearch-dsl"
    }

    async fn parse(&self, dsl: &str, _schema: Option<&Schema>) -> Vec<Diagnostic> {
        match http::extract_http_request_bodies(dsl) {
            http::HttpRequestBodies::Bodies(bodies) => {
                let mut parser = self.dsl_parser.lock().unwrap();
                let mut diagnostics = Vec::new();
                for body in bodies {
                    let mut body_diagnostics = parser.parse(&body.text);
                    http::offset_diagnostics(&mut body_diagnostics, body.start_line);
                    diagnostics.extend(body_diagnostics);
                }
                diagnostics
            }
            http::HttpRequestBodies::Diagnostics(diagnostics) => diagnostics,
            http::HttpRequestBodies::NotHttp => {
                let mut parser = self.dsl_parser.lock().unwrap();
                parser.parse(dsl)
            }
        }
    }

    async fn completion(
        &self,
        dsl: &str,
        position: Position,
        schema: Option<&Schema>,
    ) -> Vec<CompletionItem> {
        completion::complete(self, dsl, position, schema)
    }

    async fn hover(&self, dsl: &str, position: Position, schema: Option<&Schema>) -> Option<Hover> {
        let token = context::token_at_position(dsl, position);
        let schema = schema?;

        for table in &schema.tables {
            if table.name == token {
                return Some(Hover {
                    contents: tower_lsp::lsp_types::HoverContents::Scalar(MarkedString::String(
                        format!(
                            "Elasticsearch DSL Index: {}\n{}",
                            table.name,
                            table.comment.as_deref().unwrap_or("No description")
                        ),
                    )),
                    range: None,
                });
            }

            if let Some(column) = table.columns.iter().find(|column| column.name == token) {
                return Some(Hover {
                    contents: tower_lsp::lsp_types::HoverContents::Scalar(MarkedString::String(
                        format!(
                            "Elasticsearch field: {}.{}\nType: {}",
                            table.name, column.name, column.data_type
                        ),
                    )),
                    range: None,
                });
            }
        }

        None
    }

    async fn goto_definition(
        &self,
        dsl: &str,
        position: Position,
        schema: Option<&Schema>,
    ) -> Option<Location> {
        let byte_position = crate::position::lsp_position_to_byte_position(dsl, position);
        let mut parser = self.dsl_parser.lock().unwrap();
        let (tree, _) = parser.parse_with_tree(dsl);

        if let Some(ref tree) = tree {
            if let Some(node) = parser.get_node_at_position(tree, byte_position) {
                // 提取字段名
                let field_name = parser
                    .extract_field_name(node, dsl)
                    .unwrap_or_else(|| context::token_at_position(dsl, position));
                if !field_name.is_empty() {
                    // 如果是索引名或字段名，在 schema 中查找
                    if let Some(schema) = schema {
                        for table in &schema.tables {
                            if table.name == field_name {
                                return common::metadata_location(
                                    table.source_location.as_ref(),
                                    schema.source_uri.as_ref(),
                                    "file:///schema.json",
                                );
                            }
                            if let Some(column) = table
                                .columns
                                .iter()
                                .find(|column| column.name == field_name)
                            {
                                return common::metadata_location(
                                    column
                                        .source_location
                                        .as_ref()
                                        .or(table.source_location.as_ref()),
                                    schema.source_uri.as_ref(),
                                    "file:///schema.json",
                                );
                            }
                        }
                    }
                }
            }
        }

        None
    }

    async fn references(
        &self,
        dsl: &str,
        position: Position,
        _schema: Option<&Schema>,
    ) -> Vec<Location> {
        let position = crate::position::lsp_position_to_byte_position(dsl, position);
        let mut parser = self.dsl_parser.lock().unwrap();
        let (tree, _) = parser.parse_with_tree(dsl);
        let mut locations = Vec::new();

        if let Some(ref tree) = tree {
            if let Some(node) = parser.get_node_at_position(tree, position) {
                // 提取字段名
                if let Some(field_name) = parser.extract_field_name(node, dsl) {
                    // 在当前文档中查找所有引用
                    let current_uri = tower_lsp::lsp_types::Url::parse("file:///current.json")
                        .unwrap_or_else(|_| tower_lsp::lsp_types::Url::parse("file:///").unwrap());

                    // 遍历所有字段，查找匹配的
                    let root = tree.root_node();
                    let mut cursor = root.walk();
                    for child in root.children(&mut cursor) {
                        find_field_references_recursive(
                            child,
                            dsl,
                            &field_name,
                            &current_uri,
                            &mut locations,
                            &parser,
                        );
                    }
                }
            }
        }

        locations
    }

    async fn format(&self, sql: &str) -> String {
        // DSL 格式化：尝试美化 JSON
        // 这里简化处理，实际应该使用 JSON 格式化库
        serde_json::from_str::<serde_json::Value>(sql)
            .and_then(|value| serde_json::to_string_pretty(&value))
            .unwrap_or_else(|_| sql.to_string())
    }

    async fn validate(&self, sql: &str, schema: Option<&Schema>) -> Vec<Diagnostic> {
        self.parse(sql, schema).await
    }
}
