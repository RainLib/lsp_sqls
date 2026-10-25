use crate::dialect::Dialect;
use crate::dialects::common;
use crate::parser::SqlParser;
use crate::schema::Schema;
use async_trait::async_trait;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Diagnostic, Hover, Location, Position,
};

pub struct ClickHouseDialect {
    parser: std::sync::Mutex<SqlParser>,
}

impl Default for ClickHouseDialect {
    fn default() -> Self {
        Self::new()
    }
}

impl ClickHouseDialect {
    pub fn new() -> Self {
        Self {
            parser: std::sync::Mutex::new(SqlParser::new()),
        }
    }

    fn create_keyword_item(&self, keyword: &str) -> CompletionItem {
        CompletionItem {
            label: keyword.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some(format!("ClickHouse keyword: {}", keyword)),
            documentation: None,
            deprecated: None,
            preselect: None,
            sort_text: Some(format!("0{}", keyword)),
            filter_text: None,
            insert_text: Some(keyword.to_string()),
            insert_text_format: None,
            insert_text_mode: None,
            text_edit: None,
            additional_text_edits: None,
            commit_characters: None,
            command: None,
            data: None,
            tags: None,
            label_details: None,
        }
    }

    fn create_table_item(&self, table: &crate::schema::Table) -> CompletionItem {
        CompletionItem {
            label: table.name.clone(),
            kind: Some(CompletionItemKind::CLASS),
            detail: Some(format!("Table: {}", table.name)),
            documentation: table
                .comment
                .clone()
                .map(tower_lsp::lsp_types::Documentation::String),
            deprecated: None,
            preselect: None,
            sort_text: Some(format!("1{}", table.name)),
            filter_text: Some(crate::identifier_search::filter_text(&table.name, None)),
            insert_text: Some(table.name.clone()),
            insert_text_format: None,
            insert_text_mode: None,
            text_edit: None,
            additional_text_edits: None,
            commit_characters: None,
            command: None,
            data: crate::identifier_search::table_data(&table.name),
            tags: None,
            label_details: None,
        }
    }

    fn create_column_item(
        &self,
        column: &crate::schema::Column,
        table_name: Option<&str>,
    ) -> CompletionItem {
        common::create_column_item(column, table_name)
    }
}

#[async_trait]
impl Dialect for ClickHouseDialect {
    fn name(&self) -> &str {
        "clickhouse"
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
        self.completion_snapshot(sql, position, schema).items
    }

    async fn completion_with_scope(
        &self,
        sql: &str,
        position: Position,
        schema: Option<&Schema>,
        _context: Option<&tower_lsp::lsp_types::CompletionContext>,
    ) -> crate::dialect::CompletionSnapshot {
        self.completion_snapshot(sql, position, schema)
    }

    async fn hover(&self, sql: &str, position: Position, schema: Option<&Schema>) -> Option<Hover> {
        let mut parser = self.parser.lock().unwrap();
        common::schema_hover(&mut parser, sql, position, schema?, "ClickHouse")
    }

    async fn goto_definition(
        &self,
        _sql: &str,
        _position: Position,
        _schema: Option<&Schema>,
    ) -> Option<Location> {
        None
    }

    async fn references(
        &self,
        _sql: &str,
        _position: Position,
        _schema: Option<&Schema>,
    ) -> Vec<Location> {
        Vec::new()
    }

    async fn format(&self, sql: &str) -> String {
        common::format_sql_pretty(sql)
    }

    async fn validate(&self, sql: &str, schema: Option<&Schema>) -> Vec<Diagnostic> {
        self.parse(sql, schema).await
    }
}

impl ClickHouseDialect {
    fn completion_snapshot(
        &self,
        sql: &str,
        position: Position,
        schema: Option<&Schema>,
    ) -> crate::dialect::CompletionSnapshot {
        let position = SqlParser::lsp_position_to_byte_position(sql, position);
        let mut parser = self.parser.lock().unwrap();
        let syntax_tree = parser.parse_completion_tree(sql, position);
        let mut snapshot = crate::dialect::CompletionSnapshot::for_sql(
            &parser,
            syntax_tree.as_ref(),
            sql,
            position,
        );

        let context = if let Some(tree) = &syntax_tree {
            if let Some(node) = parser.get_node_at_position(tree, position) {
                parser.analyze_completion_context(node, sql, position)
            } else {
                crate::parser::CompletionContext::Default
            }
        } else {
            crate::parser::CompletionContext::Default
        };
        let is_join_clause = context == crate::parser::CompletionContext::JoinClause;

        let mut items = Vec::new();
        let mut value_policy = None;
        let keywords = &[
            "SELECT",
            "FROM",
            "WHERE",
            "INSERT",
            "INTO",
            "VALUES",
            "CREATE",
            "DROP",
            "ALTER",
            "TABLE",
            "DATABASE",
            "ENGINE",
            "MergeTree",
            "ReplacingMergeTree",
            "SummingMergeTree",
            "AggregatingMergeTree",
            "CollapsingMergeTree",
            "VersionedCollapsingMergeTree",
            "JOIN",
            "INNER",
            "LEFT",
            "RIGHT",
            "FULL",
            "OUTER",
            "ON",
            "GROUP",
            "BY",
            "ORDER",
            "HAVING",
            "QUALIFY",
            "LIMIT",
            "OFFSET",
            "UNION",
            "ALL",
            "DISTINCT",
            "AS",
            "AND",
            "OR",
            "NOT",
            "IN",
            "LIKE",
            "ILIKE",
            "BETWEEN",
            "IS",
            "NULL",
            "CAST",
            "ARRAY",
            "TUPLE",
            "MAP",
            "Nested",
            "AggregateFunction",
            "Array",
            "String",
            "Int8",
            "Int16",
            "Int32",
            "Int64",
            "UInt8",
            "UInt16",
            "UInt32",
            "UInt64",
            "Float32",
            "Float64",
            "Date",
            "DateTime",
        ];

        match context {
            crate::parser::CompletionContext::FromClause
            | crate::parser::CompletionContext::JoinClause => {
                let join_keywords: Vec<&str> = keywords
                    .iter()
                    .filter(|&&k| matches!(k, "JOIN" | "INNER" | "LEFT" | "RIGHT" | "OUTER" | "ON"))
                    .copied()
                    .collect();
                for keyword in join_keywords {
                    items.push(self.create_keyword_item(keyword));
                }
                if let Some(schema) = schema {
                    let prefix = common::cursor_prefix_excluding_keywords(
                        sql,
                        position,
                        &["from", "join", "inner", "left", "right", "full", "cross"],
                    );
                    let qualified = common::cursor_has_identifier_qualifier(sql, position);
                    common::add_schema_tables(&mut items, schema, &prefix, qualified, qualified);
                    if is_join_clause {
                        if let Some(tree) = &syntax_tree {
                            let referenced_tables =
                                parser.extract_referenced_tables_at_position(tree, sql, position);
                            let aliases = parser.extract_aliases_at_position(tree, sql, position);
                            common::add_foreign_key_join_snippets(
                                &mut items,
                                schema,
                                &referenced_tables,
                                &aliases,
                                "",
                                false,
                            );
                        }
                    }
                }
            }
            crate::parser::CompletionContext::FromContinuationClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in [
                    "AS",
                    "JOIN",
                    "INNER JOIN",
                    "LEFT JOIN",
                    "RIGHT JOIN",
                    "FULL JOIN",
                    "CROSS JOIN",
                    "WHERE",
                    "GROUP BY",
                    "QUALIFY",
                    "ORDER BY",
                    "LIMIT",
                    "OFFSET",
                ] {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let mut item = self.create_keyword_item(keyword);
                    common::set_completion_sort_text(&mut item, "0", keyword);
                    items.push(item);
                }
            }
            crate::parser::CompletionContext::JoinConditionClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in ["AS", "ON", "USING"] {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let mut item = self.create_keyword_item(keyword);
                    common::set_completion_sort_text(&mut item, "0", keyword);
                    items.push(item);
                }
            }
            crate::parser::CompletionContext::SelectClause => {
                let prefix = common::cursor_prefix_excluding_keywords(
                    sql,
                    position,
                    &["select", "distinct"],
                );
                let select_keywords: Vec<&str> = keywords
                    .iter()
                    .filter(|&&k| matches!(k, "SELECT" | "DISTINCT" | "AS" | "FROM"))
                    .copied()
                    .collect();
                for keyword in select_keywords {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    items.push(self.create_keyword_item(keyword));
                }
                if let (Some(schema), Some(tree)) = (schema, &syntax_tree) {
                    let referenced_tables = common::referenced_table_names_at_position(
                        &parser, tree, sql, position, schema,
                    );
                    common::add_schema_columns(
                        &mut items,
                        schema,
                        &referenced_tables,
                        referenced_tables.len() != 1,
                        &prefix,
                        "0",
                    );
                }
            }
            crate::parser::CompletionContext::SelectContinuationClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in common::select_continuation_keywords(sql, position)
                    .into_iter()
                    .chain(["QUALIFY"])
                {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let mut item = self.create_keyword_item(keyword);
                    common::set_completion_sort_text(&mut item, "0", keyword);
                    items.push(item);
                }
            }
            crate::parser::CompletionContext::WhereClause => {
                let prefix = common::cursor_prefix_excluding_keywords(
                    sql,
                    position,
                    &["where", "and", "or", "not"],
                );
                let predicate_operator_expected =
                    common::predicate_operator_expected(sql, position);
                let latest_predicate_clause = common::latest_predicate_clause(sql, position);
                let where_keywords: Vec<&str> = keywords
                    .iter()
                    .filter(|&&k| {
                        if predicate_operator_expected {
                            matches!(
                                k,
                                "AND"
                                    | "OR"
                                    | "NOT"
                                    | "IN"
                                    | "LIKE"
                                    | "ILIKE"
                                    | "BETWEEN"
                                    | "IS"
                                    | "NULL"
                            )
                        } else {
                            matches!(k, "NOT" | "NULL")
                        }
                    })
                    .copied()
                    .collect();

                if !predicate_operator_expected {
                    if let (Some(schema), Some(tree)) = (schema, &syntax_tree) {
                        let referenced_tables = common::referenced_table_names_at_position(
                            &parser, tree, sql, position, schema,
                        );
                        let use_table_prefix =
                            !matches!(latest_predicate_clause, Some("SET" | "UPDATE"))
                                && referenced_tables.len() != 1;
                        value_policy = common::add_column_value_items(
                            &mut items,
                            schema,
                            &referenced_tables,
                            sql,
                            position,
                            &prefix,
                            "clickhouse",
                        );
                        common::add_schema_columns(
                            &mut items,
                            schema,
                            &referenced_tables,
                            use_table_prefix,
                            &prefix,
                            "0",
                        );
                    }
                }

                for keyword in where_keywords {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let mut item = self.create_keyword_item(keyword);
                    item.sort_text = Some(format!("1{}", keyword));
                    items.push(item);
                }

                if predicate_operator_expected {
                    let operators = vec!["=", "<>", "!=", ">", "<", ">=", "<="];
                    for op in operators {
                        items.push(CompletionItem {
                            label: op.to_string(),
                            kind: Some(CompletionItemKind::OPERATOR),
                            detail: Some(format!("Operator: {}", op)),
                            documentation: None,
                            deprecated: None,
                            preselect: None,
                            sort_text: Some(format!("1{}", op)),
                            filter_text: None,
                            insert_text: Some(op.to_string()),
                            insert_text_format: None,
                            insert_text_mode: None,
                            text_edit: None,
                            additional_text_edits: None,
                            commit_characters: None,
                            command: None,
                            data: None,
                            tags: None,
                            label_details: None,
                        });
                    }
                }
            }
            crate::parser::CompletionContext::OrderByClause => {
                let keywords_list: Vec<&str> = keywords
                    .iter()
                    .filter(|&&k| matches!(k, "ASC" | "DESC" | "BY"))
                    .copied()
                    .collect();
                for keyword in keywords_list {
                    items.push(self.create_keyword_item(keyword));
                }
                if let Some(schema) = schema {
                    for table in &schema.tables {
                        for column in &table.columns {
                            items.push(self.create_column_item(
                                column,
                                Some(&format!("{}.{}", schema.database, table.name)),
                            ));
                        }
                    }
                }
            }
            crate::parser::CompletionContext::GroupByClause => {
                if let Some(schema) = schema {
                    for table in &schema.tables {
                        for column in &table.columns {
                            items.push(self.create_column_item(
                                column,
                                Some(&format!("{}.{}", schema.database, table.name)),
                            ));
                        }
                    }
                }
            }
            crate::parser::CompletionContext::GroupByContinuationClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in [",", "HAVING", "ORDER BY", "LIMIT", "OFFSET", "WITH TOTALS"] {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let mut item = self.create_keyword_item(keyword);
                    common::set_completion_sort_text(
                        &mut item,
                        common::group_by_continuation_sort_prefix(keyword),
                        keyword,
                    );
                    items.push(item);
                }
            }
            crate::parser::CompletionContext::OrderDirectionClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in common::order_direction_keywords(
                    sql,
                    position,
                    false,
                    &[",", "LIMIT", "OFFSET", "WITH FILL"],
                ) {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let mut item = self.create_keyword_item(keyword);
                    common::set_completion_sort_text(
                        &mut item,
                        common::order_direction_sort_prefix(keyword),
                        keyword,
                    );
                    items.push(item);
                }
            }
            crate::parser::CompletionContext::HavingClause => {
                let prefix = common::cursor_prefix_excluding_keywords(
                    sql,
                    position,
                    &["having", "and", "or", "not"],
                );
                let predicate_operator_expected =
                    common::predicate_operator_expected(sql, position);
                let having_keywords: Vec<&str> = keywords
                    .iter()
                    .filter(|&&k| {
                        if predicate_operator_expected {
                            matches!(
                                k,
                                "AND"
                                    | "OR"
                                    | "NOT"
                                    | "IN"
                                    | "LIKE"
                                    | "ILIKE"
                                    | "BETWEEN"
                                    | "IS"
                                    | "NULL"
                            )
                        } else {
                            matches!(k, "NOT" | "NULL")
                        }
                    })
                    .copied()
                    .collect();

                if !predicate_operator_expected {
                    let aggregate_functions = vec!["COUNT", "SUM", "AVG", "MIN", "MAX"];
                    for func in aggregate_functions {
                        if !prefix.is_empty() && !func.to_lowercase().starts_with(&prefix) {
                            continue;
                        }
                        items.push(self.create_keyword_item(func));
                    }
                    if let (Some(schema), Some(tree)) = (schema, &syntax_tree) {
                        let referenced_tables = common::referenced_table_names_at_position(
                            &parser, tree, sql, position, schema,
                        );
                        let use_table_prefix = referenced_tables.len() != 1;
                        common::add_schema_columns(
                            &mut items,
                            schema,
                            &referenced_tables,
                            use_table_prefix,
                            &prefix,
                            "0",
                        );
                    }
                }

                for keyword in having_keywords {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    items.push(self.create_keyword_item(keyword));
                }

                if predicate_operator_expected {
                    let operators = vec!["=", "<>", "!=", ">", "<", ">=", "<="];
                    common::add_operator_items(&mut items, &operators, &prefix, "1");
                }
            }
            crate::parser::CompletionContext::UsingClause => {
                let prefix = common::cursor_prefix_excluding_keywords(sql, position, &["using"]);
                if let (Some(schema), Some(tree)) = (schema, &syntax_tree) {
                    let referenced_tables = common::referenced_table_names_at_position(
                        &parser, tree, sql, position, schema,
                    );
                    common::add_schema_using_columns(
                        &mut items,
                        schema,
                        &referenced_tables,
                        &prefix,
                        "0",
                    );
                }
            }
            crate::parser::CompletionContext::ReferenceColumnClause => {
                let prefix = common::cursor_prefix(sql, position);
                if let Some(schema) = schema {
                    if let Some(table_reference) =
                        SqlParser::reference_table_at_position(sql, position)
                    {
                        common::add_reference_table_columns(
                            &mut items,
                            schema,
                            &table_reference,
                            &prefix,
                            "0",
                        );
                    }
                }
            }
            crate::parser::CompletionContext::ReferenceActionClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in ["(", "ON DELETE", "ON UPDATE"] {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    items.push(self.create_keyword_item(keyword));
                }
            }
            crate::parser::CompletionContext::ReferenceRuleClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in ["CASCADE", "RESTRICT", "NO ACTION", "SET NULL"] {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    items.push(self.create_keyword_item(keyword));
                }
            }
            crate::parser::CompletionContext::ColumnTargetClause => {
                let prefix = common::cursor_prefix_excluding_keywords(sql, position, &["column"]);
                if let (Some(schema), Some(tree)) = (schema, &syntax_tree) {
                    let referenced_tables = common::referenced_table_names_at_position(
                        &parser, tree, sql, position, schema,
                    );
                    common::add_schema_columns(
                        &mut items,
                        schema,
                        &referenced_tables,
                        false,
                        &prefix,
                        "0",
                    );
                }
            }
            crate::parser::CompletionContext::ConstraintTargetClause => {
                let prefix =
                    common::cursor_prefix_excluding_keywords(sql, position, &["constraint"]);
                if let (Some(schema), Some(tree)) = (schema, &syntax_tree) {
                    let referenced_tables = common::referenced_table_names_at_position(
                        &parser, tree, sql, position, schema,
                    );
                    common::add_schema_constraints(
                        &mut items,
                        schema,
                        &referenced_tables,
                        &prefix,
                        "0",
                    );
                }
            }
            crate::parser::CompletionContext::AlterTableActionClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in [
                    "ADD COLUMN",
                    "DROP COLUMN",
                    "MODIFY COLUMN",
                    "RENAME COLUMN",
                    "RENAME TO",
                ] {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    items.push(self.create_keyword_item(keyword));
                }
            }
            crate::parser::CompletionContext::InsertActionClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in ["(", "VALUES", "SELECT", "FORMAT"] {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    items.push(self.create_keyword_item(keyword));
                }
            }
            crate::parser::CompletionContext::InsertValueClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in ["DEFAULT", "NULL", "TRUE", "FALSE", "now()", "today()"] {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let mut item = self.create_keyword_item(keyword);
                    common::set_completion_sort_text(&mut item, "0", keyword);
                    items.push(item);
                }
            }
            crate::parser::CompletionContext::InsertContinuationClause => {}
            crate::parser::CompletionContext::InsertConflictTargetClause
            | crate::parser::CompletionContext::InsertConflictConstraintClause
            | crate::parser::CompletionContext::InsertConflictActionClause => {}
            crate::parser::CompletionContext::ExpressionValueClause => {
                let prefix = common::cursor_prefix(sql, position);
                if let Some(schema) = schema {
                    let referenced_tables = syntax_tree
                        .as_ref()
                        .map(|tree| {
                            common::referenced_table_names_at_position(
                                &parser, tree, sql, position, schema,
                            )
                        })
                        .unwrap_or_default();
                    value_policy = common::add_column_value_items(
                        &mut items,
                        schema,
                        &referenced_tables,
                        sql,
                        position,
                        &prefix,
                        "clickhouse",
                    );
                }
                let mut keywords = vec!["NULL", "TRUE", "FALSE", "now()", "today()"];
                if common::expression_value_allows_default(sql, position) {
                    keywords.insert(0, "DEFAULT");
                }
                for keyword in keywords {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let mut item = self.create_keyword_item(keyword);
                    common::set_completion_sort_text(&mut item, "0", keyword);
                    items.push(item);
                }
            }
            crate::parser::CompletionContext::CaseResultClause => {
                let prefix =
                    common::cursor_prefix_excluding_keywords(sql, position, &["then", "else"]);
                if let Some(schema) = schema {
                    for table in &schema.tables {
                        for column in &table.columns {
                            if !prefix.is_empty()
                                && crate::identifier_search::column_match_rank(column, &prefix)
                                    .is_none()
                            {
                                continue;
                            }
                            let mut item = self.create_column_item(
                                column,
                                Some(&format!("{}.{}", schema.database, table.name)),
                            );
                            common::set_completion_sort_text(&mut item, "0", &column.name);
                            items.push(item);
                        }
                    }
                }
                for keyword in ["NULL", "TRUE", "FALSE", "now()", "today()"] {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let mut item = self.create_keyword_item(keyword);
                    common::set_completion_sort_text(&mut item, "1", keyword);
                    items.push(item);
                }
            }
            crate::parser::CompletionContext::CaseWhenValueContinuationClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in ["THEN"] {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let mut item = self.create_keyword_item(keyword);
                    common::set_completion_sort_text(&mut item, "0", keyword);
                    items.push(item);
                }
            }
            crate::parser::CompletionContext::CaseContinuationClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in common::case_continuation_keywords(sql, position) {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let mut item = self.create_keyword_item(keyword);
                    common::set_completion_sort_text(&mut item, "0", keyword);
                    items.push(item);
                }
            }
            crate::parser::CompletionContext::PredicateContinuationClause => {
                let prefix = common::cursor_prefix(sql, position);
                let mut keywords = common::predicate_continuation_keywords(sql, position, false);
                if keywords.contains(&"ORDER BY")
                    && SqlParser::qualify_query_range(
                        sql,
                        SqlParser::byte_offset_for_position(sql, position),
                    )
                    .is_none()
                {
                    keywords.push("QUALIFY");
                }
                for keyword in keywords {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let mut item = self.create_keyword_item(keyword);
                    common::set_completion_sort_text(&mut item, "0", keyword);
                    items.push(item);
                }
            }
            crate::parser::CompletionContext::UpdateActionClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in ["SET", "WHERE"] {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    items.push(self.create_keyword_item(keyword));
                }
            }
            crate::parser::CompletionContext::DeleteActionClause => {
                let prefix = common::cursor_prefix(sql, position);
                for keyword in ["WHERE"] {
                    if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    items.push(self.create_keyword_item(keyword));
                }
            }
            crate::parser::CompletionContext::IndexTargetClause => {
                let prefix = common::cursor_prefix_excluding_keywords(sql, position, &["index"]);
                if let (Some(schema), Some(tree)) = (schema, &syntax_tree) {
                    let referenced_tables = common::referenced_table_names_at_position(
                        &parser, tree, sql, position, schema,
                    );
                    common::add_schema_indexes(
                        &mut items,
                        schema,
                        &referenced_tables,
                        &prefix,
                        "0",
                        false,
                    );
                }
            }
            crate::parser::CompletionContext::DataTypeClause => {
                let prefix = common::cursor_prefix(sql, position);
                for data_type in [
                    "String", "Int32", "Int64", "UInt32", "UInt64", "Float32", "Float64", "Bool",
                    "Date", "DateTime", "Decimal", "Array()", "Map()", "Tuple()",
                ] {
                    if !prefix.is_empty() && !data_type.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    items.push(self.create_keyword_item(data_type));
                }
            }
            crate::parser::CompletionContext::TableColumn => {
                if let Some(tree) = &syntax_tree {
                    if let Some(table_name) =
                        common::table_column_reference_at_position(&parser, tree, sql, position)
                    {
                        if let Some(schema) = schema {
                            let aliases = snapshot.relation_aliases.clone().unwrap_or_else(|| {
                                SqlParser::relation_aliases_at_position(sql, position)
                            });
                            let real_table_name =
                                SqlParser::resolve_relation_alias(&aliases, &table_name)
                                    .map(|alias| alias.relation.as_str())
                                    .unwrap_or(table_name.as_str());
                            if let Some(table) =
                                common::find_table_by_reference(schema, real_table_name)
                            {
                                for column in &table.columns {
                                    items.push(self.create_column_item(column, None));
                                }
                            }
                        }
                    }
                }
            }
            crate::parser::CompletionContext::Default => {
                for keyword in keywords {
                    items.push(self.create_keyword_item(keyword));
                }
                if let Some(schema) = schema {
                    for table in &schema.tables {
                        items.push(self.create_table_item(table));
                    }
                }
            }
        }

        if let (Some(schema), Some(tree)) = (schema, syntax_tree.as_ref()) {
            let aliases = snapshot
                .relation_aliases
                .clone()
                .unwrap_or_else(|| SqlParser::relation_aliases_at_position(sql, position));
            common::apply_column_aliases(&mut items, schema, &aliases);
            common::apply_scoped_column_completion(
                &mut items, &parser, tree, sql, position, schema, &context,
            );
        }

        common::apply_value_completion_policy(&mut items, value_policy, "clickhouse");
        snapshot.items = items;
        snapshot
    }
}
