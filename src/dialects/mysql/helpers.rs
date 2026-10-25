use super::MysqlDialect;
use crate::dialects::common;
use crate::parser::SqlParser;
use crate::placeholder::SqlPlaceholderDialect;
use crate::schema::{Function, Schema};
use tower_lsp::lsp_types::{CompletionItem, Position};

impl MysqlDialect {
    pub(crate) fn sqlite() -> Self {
        Self {
            value_profile: "sqlite",
            parser: std::sync::Mutex::new(
                SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::Mysql)
                    .with_sqlite_joins(),
            ),
        }
    }
    pub fn new() -> Self {
        Self {
            value_profile: "mysql",
            parser: std::sync::Mutex::new(SqlParser::new_with_placeholder_dialect(
                SqlPlaceholderDialect::Mysql,
            )),
        }
    }

    /// 创建关键字补全项
    pub(super) fn create_keyword_item(&self, keyword: &str) -> CompletionItem {
        common::create_keyword_item("MySQL", keyword)
    }

    /// 创建列补全项
    pub(super) fn create_column_item(
        &self,
        column: &crate::schema::Column,
        table_name: Option<&str>,
    ) -> CompletionItem {
        common::create_column_item(column, table_name)
    }

    pub(super) fn add_schema_functions(
        &self,
        items: &mut Vec<CompletionItem>,
        schema: &Schema,
        prefix: &str,
        sort_prefix: &str,
        qualify_with_database: bool,
    ) {
        common::add_schema_functions(items, schema, prefix, sort_prefix, qualify_with_database);
    }

    pub(super) fn table_matches(
        schema: &Schema,
        table: &crate::schema::Table,
        reference: &str,
    ) -> bool {
        common::table_matches(schema, table, reference)
    }

    pub(super) fn find_table_by_reference<'a>(
        schema: &'a Schema,
        reference: &str,
    ) -> Option<&'a crate::schema::Table> {
        common::find_table_by_reference(schema, reference)
    }

    pub(super) fn referenced_table_names_at_position(
        parser: &SqlParser,
        tree: &tree_sitter::Tree,
        sql: &str,
        position: Position,
        schema: &Schema,
    ) -> Vec<String> {
        common::referenced_table_names_at_position(parser, tree, sql, position, schema)
    }

    pub(super) fn find_function_by_reference<'a>(
        schema: &'a Schema,
        reference: &str,
    ) -> Option<&'a Function> {
        common::find_function_by_reference(schema, reference)
    }

    pub(super) fn is_function_reference(node: tree_sitter::Node, sql: &str) -> bool {
        common::is_function_reference(node, sql)
    }

    pub(super) fn cursor_prefix(sql: &str, position: Position) -> String {
        common::cursor_prefix(sql, position)
    }

    pub(super) fn relation_reference_prefix(sql: &str, position: Position) -> String {
        common::cursor_prefix_excluding_keywords(
            sql,
            position,
            &[
                "from", "join", "inner", "left", "right", "full", "outer", "cross", "insert",
                "into", "update", "delete", "truncate", "alter", "drop", "table", "view", "on",
            ],
        )
    }

    pub(super) fn cursor_has_identifier_qualifier(sql: &str, position: Position) -> bool {
        common::cursor_has_identifier_qualifier(sql, position)
    }

    pub(super) fn add_schema_columns(
        &self,
        items: &mut Vec<CompletionItem>,
        schema: &Schema,
        referenced_tables: &[String],
        use_table_prefix: bool,
        prefix: &str,
        sort_prefix: &str,
    ) {
        common::add_schema_columns(
            items,
            schema,
            referenced_tables,
            use_table_prefix,
            prefix,
            sort_prefix,
        );
    }
}
