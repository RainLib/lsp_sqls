use crate::dialect::Dialect;
use crate::dialects::common;
use crate::parser::SqlParser;
use crate::schema::Schema;
use async_trait::async_trait;
use tower_lsp::lsp_types::{CompletionItem, Diagnostic, Hover, Location, Position};

pub struct PostgresDialect {
    pub(super) identity: &'static str,
    pub(super) parser: std::sync::Mutex<SqlParser>,
}

impl Default for PostgresDialect {
    fn default() -> Self {
        Self::new()
    }
}

mod completion;
mod helpers;
mod hover;

#[async_trait]
impl Dialect for PostgresDialect {
    fn name(&self) -> &str {
        self.identity
    }

    async fn parse(&self, sql: &str, schema: Option<&Schema>) -> Vec<Diagnostic> {
        self.parse_with_syntax(sql, schema).await.diagnostics
    }

    async fn parse_with_syntax(
        &self,
        sql: &str,
        _schema: Option<&Schema>,
    ) -> crate::parser::ParseResult {
        common::validate_join_profile(self.parser.lock().unwrap().parse(sql), self.name())
    }

    async fn completion(
        &self,
        sql: &str,
        position: Position,
        schema: Option<&Schema>,
    ) -> Vec<CompletionItem> {
        completion::completion(self, sql, position, schema).items
    }

    async fn completion_with_scope(
        &self,
        sql: &str,
        position: Position,
        schema: Option<&Schema>,
        _context: Option<&tower_lsp::lsp_types::CompletionContext>,
    ) -> crate::dialect::CompletionSnapshot {
        completion::completion(self, sql, position, schema)
    }

    async fn hover(&self, sql: &str, position: Position, schema: Option<&Schema>) -> Option<Hover> {
        hover::hover(self, sql, position, schema)
    }

    async fn goto_definition(
        &self,
        sql: &str,
        position: Position,
        schema: Option<&Schema>,
    ) -> Option<Location> {
        let mut parser = self.parser.lock().unwrap();
        let parse_result = parser.parse(sql);

        if let Some(tree) = &parse_result.tree {
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

                let is_column = node_kind == "column_name"
                    || node_kind == "column_reference"
                    || node_kind == "column_identifier"
                    || (node_kind == "identifier" && parser.is_in_column_context(node, sql));

                if is_table {
                    if let Some(schema) = schema {
                        let relation = crate::dialects::common::relation_reference_node(node);
                        let table_ref = parser.node_text(relation, sql);
                        if let Some(table) = Self::find_table_by_reference(schema, &table_ref) {
                            return common::metadata_location(
                                table.source_location.as_ref(),
                                schema.source_uri.as_ref(),
                                "file:///schema.sql",
                            );
                        }
                    }
                }

                if is_column {
                    if let Some(schema) = schema {
                        let (table_name, column_name) =
                            if let Some(table_name) = parser.get_table_name_for_column(node, sql) {
                                (
                                    Some(table_name),
                                    SqlParser::identifier_last_part(&node_text),
                                )
                            } else {
                                let tables = parser.extract_tables(tree, sql);
                                (
                                    tables.first().cloned(),
                                    SqlParser::identifier_last_part(&node_text),
                                )
                            };

                        for table in &schema.tables {
                            if let Some(ref tname) = table_name {
                                if Self::table_matches(schema, table, tname) {
                                    if let Some(column) = table
                                        .columns
                                        .iter()
                                        .find(|column| column.name == column_name)
                                    {
                                        return common::metadata_location(
                                            column
                                                .source_location
                                                .as_ref()
                                                .or(table.source_location.as_ref()),
                                            schema.source_uri.as_ref(),
                                            "file:///schema.sql",
                                        );
                                    }
                                }
                            } else if let Some(column) = table
                                .columns
                                .iter()
                                .find(|column| column.name == column_name)
                            {
                                return common::metadata_location(
                                    column
                                        .source_location
                                        .as_ref()
                                        .or(table.source_location.as_ref()),
                                    schema.source_uri.as_ref(),
                                    "file:///schema.sql",
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
        sql: &str,
        position: Position,
        _schema: Option<&Schema>,
    ) -> Vec<Location> {
        let mut parser = self.parser.lock().unwrap();
        let parse_result = parser.parse(sql);

        let mut locations = Vec::new();

        if let Some(tree) = &parse_result.tree {
            if let Some(node) = parser.get_node_at_position(tree, position) {
                let identifier = parser.node_text(node, sql);
                let node_kind = node.kind();

                if crate::token::Keywords::is_keyword(&identifier)
                    || crate::token::Operators::is_operator(&identifier)
                    || crate::token::Delimiters::is_delimiter(&identifier)
                {
                    return locations;
                }

                let is_table = node_kind == "table_name"
                    || node_kind == "table_reference"
                    || node_kind == "table_identifier"
                    || (node_kind == "identifier" && parser.is_in_from_context(node, sql));

                let is_column = node_kind == "column_name"
                    || node_kind == "column_reference"
                    || node_kind == "column_identifier"
                    || (node_kind == "identifier" && parser.is_in_column_context(node, sql));

                if is_table || is_column {
                    let tokens = parser.tokenize(tree, sql);
                    let current_uri = tower_lsp::lsp_types::Url::parse("file:///current.sql")
                        .unwrap_or_else(|_| tower_lsp::lsp_types::Url::parse("file:///").unwrap());

                    for token in tokens {
                        if token.text.eq_ignore_ascii_case(&identifier)
                            && !crate::token::Keywords::is_keyword(&token.text)
                            && !crate::token::Operators::is_operator(&token.text)
                            && !crate::token::Delimiters::is_delimiter(&token.text)
                        {
                            locations.push(Location {
                                uri: current_uri.clone(),
                                range: tower_lsp::lsp_types::Range {
                                    start: token.position,
                                    end: tower_lsp::lsp_types::Position {
                                        line: token.position.line,
                                        character: token.position.character
                                            + token.text.encode_utf16().count() as u32,
                                    },
                                },
                            });
                        }
                    }
                }
            }
        }

        locations
    }

    async fn format(&self, sql: &str) -> String {
        common::format_sql_pretty(sql)
    }

    async fn validate(&self, sql: &str, schema: Option<&Schema>) -> Vec<Diagnostic> {
        self.parse(sql, schema).await
    }
}
