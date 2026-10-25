use super::MysqlDialect;
use crate::dialects::common;
use crate::parser::SqlParser;
use crate::schema::Schema;
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position};

pub(super) fn completion(
    dialect: &MysqlDialect,
    sql: &str,
    position: Position,
    schema: Option<&Schema>,
) -> crate::dialect::CompletionSnapshot {
    let position = SqlParser::lsp_position_to_byte_position(sql, position);
    let mut parser = dialect.parser.lock().unwrap();
    let syntax_tree = parser.parse_completion_tree(sql, position);
    let mut snapshot =
        crate::dialect::CompletionSnapshot::for_sql(&parser, syntax_tree.as_ref(), sql, position);

    // 分析补全上下文
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

    // 根据上下文提供不同的补全
    match context {
        crate::parser::CompletionContext::FromClause
        | crate::parser::CompletionContext::JoinClause => {
            let prefix = MysqlDialect::relation_reference_prefix(sql, position);
            if let Some(schema) = schema {
                let qualify_with_database =
                    MysqlDialect::cursor_has_identifier_qualifier(sql, position);
                common::add_schema_tables(
                    &mut items,
                    schema,
                    &prefix,
                    qualify_with_database,
                    qualify_with_database,
                );
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
                            &prefix,
                            qualify_with_database,
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
                "CROSS JOIN",
                "WHERE",
                "GROUP BY",
                "ORDER BY",
                "LIMIT",
                "OFFSET",
            ] {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
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
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "0", keyword);
                items.push(item);
            }
        }

        crate::parser::CompletionContext::SelectClause => {
            let prefix =
                common::cursor_prefix_excluding_keywords(sql, position, &["select", "distinct"]);
            if let Some(schema) = schema {
                let referenced_tables = syntax_tree
                    .as_ref()
                    .map(|tree| {
                        MysqlDialect::referenced_table_names_at_position(
                            &parser, tree, sql, position, schema,
                        )
                    })
                    .unwrap_or_default();
                let use_table_prefix = referenced_tables.len() != 1;

                dialect.add_schema_columns(
                    &mut items,
                    schema,
                    &referenced_tables,
                    use_table_prefix,
                    &prefix,
                    "0",
                );
                dialect.add_schema_functions(
                    &mut items,
                    schema,
                    &prefix,
                    "1",
                    MysqlDialect::cursor_has_identifier_qualifier(sql, position),
                );
            }

            // 添加聚合函数和常用函数（优先级中等）
            let functions = vec![
                ("COUNT", "Aggregate function: count rows"),
                ("SUM", "Aggregate function: sum values"),
                ("AVG", "Aggregate function: average"),
                ("MIN", "Aggregate function: minimum"),
                ("MAX", "Aggregate function: maximum"),
                ("CONCAT", "String function: concatenate"),
                ("UPPER", "String function: uppercase"),
                ("LOWER", "String function: lowercase"),
                ("NOW", "Date function: current timestamp"),
                ("DATE", "Date function: extract date"),
            ];

            for (func, desc) in functions {
                // Apply prefix filter to functions
                if !prefix.is_empty() && !func.to_lowercase().starts_with(&prefix) {
                    continue;
                }

                items.push(CompletionItem {
                    label: func.to_string(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some(desc.to_string()),
                    documentation: None,
                    deprecated: None,
                    preselect: None,
                    sort_text: Some(common::completion_sort_text("1", func)), // Functions after columns
                    filter_text: None,
                    insert_text: Some(format!("{}()", func)),
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

            // 然后添加 SELECT 相关关键字（优先级最低）
            let select_keywords = vec!["DISTINCT", "AS", "FROM"];
            for keyword in select_keywords {
                // Apply prefix filter to keywords
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }

                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "2", keyword); // Keywords last
                items.push(item);
            }
        }

        crate::parser::CompletionContext::SelectContinuationClause => {
            let prefix = common::cursor_prefix(sql, position);
            for keyword in common::select_continuation_keywords(sql, position) {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
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
            let predicate_operator_expected = common::predicate_operator_expected(sql, position);
            let latest_predicate_clause = common::latest_predicate_clause(sql, position);
            if !predicate_operator_expected {
                if let Some(schema) = schema {
                    if let Some(tree) = &syntax_tree {
                        let referenced_tables = MysqlDialect::referenced_table_names_at_position(
                            &parser, tree, sql, position, schema,
                        );
                        let use_table_prefix =
                            !matches!(latest_predicate_clause, Some("SET" | "UPDATE"))
                                && referenced_tables.len() > 1;
                        value_policy = common::add_column_value_items(
                            &mut items,
                            schema,
                            &referenced_tables,
                            sql,
                            position,
                            &prefix,
                            dialect.value_profile,
                        );
                        dialect.add_schema_columns(
                            &mut items,
                            schema,
                            &referenced_tables,
                            use_table_prefix,
                            &prefix,
                            "0",
                        );
                    }
                    dialect.add_schema_functions(
                        &mut items,
                        schema,
                        &prefix,
                        "1",
                        MysqlDialect::cursor_has_identifier_qualifier(sql, position),
                    );
                }
            }

            if predicate_operator_expected {
                let operators = vec![
                    "=",
                    "<>",
                    "!=",
                    ">",
                    "<",
                    ">=",
                    "<=",
                    "LIKE",
                    "IN",
                    "BETWEEN",
                    "IS NULL",
                    "IS NOT NULL",
                ];
                common::add_operator_items(&mut items, &operators, &prefix, "1");
            }
        }

        crate::parser::CompletionContext::OrderByClause => {
            let prefix = common::cursor_prefix_excluding_keywords(sql, position, &["order", "by"]);
            // ORDER BY：补全列名和排序关键字
            // 添加列名补全 (优先级高)
            if let Some(schema) = schema {
                // Check if query has multiple tables (to decide whether to use table prefix)
                if let Some(tree) = &syntax_tree {
                    let referenced_tables = MysqlDialect::referenced_table_names_at_position(
                        &parser, tree, sql, position, schema,
                    );
                    let use_table_prefix = referenced_tables.len() != 1;
                    dialect.add_schema_columns(
                        &mut items,
                        schema,
                        &referenced_tables,
                        use_table_prefix,
                        &prefix,
                        "0",
                    );
                } else {
                    // 如果没有解析树，默认返回所有列，不带前缀
                    dialect.add_schema_columns(&mut items, schema, &[], false, &prefix, "0");
                }
            }

            // 添加 ORDER BY 排序关键字 (优先级低)
            let keywords = vec!["ASC", "DESC"];
            for keyword in keywords {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "1", keyword); // Keywords after columns
                items.push(item);
            }
        }

        crate::parser::CompletionContext::OrderDirectionClause => {
            let prefix = common::cursor_prefix(sql, position);
            for keyword in
                common::order_direction_keywords(sql, position, false, &[",", "LIMIT", "OFFSET"])
            {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(
                    &mut item,
                    common::order_direction_sort_prefix(keyword),
                    keyword,
                );
                items.push(item);
            }
        }

        crate::parser::CompletionContext::GroupByClause => {
            let prefix = common::cursor_prefix_excluding_keywords(sql, position, &["group", "by"]);
            // GROUP BY：只补全列名，不需要排序关键字
            // 添加列名补全
            if let Some(schema) = schema {
                // Check if query has multiple tables (to decide whether to use table prefix)
                if let Some(tree) = &syntax_tree {
                    let referenced_tables = MysqlDialect::referenced_table_names_at_position(
                        &parser, tree, sql, position, schema,
                    );
                    let use_table_prefix = referenced_tables.len() != 1;
                    dialect.add_schema_columns(
                        &mut items,
                        schema,
                        &referenced_tables,
                        use_table_prefix,
                        &prefix,
                        "0",
                    );
                } else {
                    // 如果没有解析树，默认返回所有列，不带前缀
                    dialect.add_schema_columns(&mut items, schema, &[], false, &prefix, "0");
                }
            }
            // GROUP BY 不添加任何关键字
        }

        crate::parser::CompletionContext::GroupByContinuationClause => {
            let prefix = common::cursor_prefix(sql, position);
            for keyword in [",", "HAVING", "ORDER BY", "LIMIT", "OFFSET", "WITH ROLLUP"] {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(
                    &mut item,
                    common::group_by_continuation_sort_prefix(keyword),
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
            let predicate_operator_expected = common::predicate_operator_expected(sql, position);

            if !predicate_operator_expected {
                if let Some(schema) = schema {
                    if let Some(tree) = &syntax_tree {
                        let referenced_tables = MysqlDialect::referenced_table_names_at_position(
                            &parser, tree, sql, position, schema,
                        );
                        let use_table_prefix = referenced_tables.len() != 1;
                        dialect.add_schema_columns(
                            &mut items,
                            schema,
                            &referenced_tables,
                            use_table_prefix,
                            &prefix,
                            "0",
                        );
                    }
                    dialect.add_schema_functions(
                        &mut items,
                        schema,
                        &prefix,
                        "1",
                        MysqlDialect::cursor_has_identifier_qualifier(sql, position),
                    );
                }

                let aggregate_functions = vec!["COUNT", "SUM", "AVG", "MIN", "MAX"];
                for func in aggregate_functions {
                    if !prefix.is_empty() && !func.to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let mut item = dialect.create_keyword_item(func);
                    item.kind = Some(CompletionItemKind::FUNCTION);
                    common::set_completion_sort_text(&mut item, "1", func);
                    items.push(item);
                }
            }

            let having_keywords = if predicate_operator_expected {
                vec![
                    "AND", "OR", "NOT", "IN", "LIKE", "BETWEEN", "IS", "NULL", "TRUE", "FALSE",
                ]
            } else {
                vec!["NOT", "TRUE", "FALSE"]
            };
            for keyword in having_keywords {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "2", keyword);
                items.push(item);
            }

            if predicate_operator_expected {
                let operators = vec![
                    "=",
                    "<>",
                    "!=",
                    ">",
                    "<",
                    ">=",
                    "<=",
                    "LIKE",
                    "IN",
                    "BETWEEN",
                    "IS NULL",
                    "IS NOT NULL",
                ];
                common::add_operator_items(&mut items, &operators, &prefix, "1");
            }
        }

        crate::parser::CompletionContext::UsingClause => {
            let prefix = common::cursor_prefix_excluding_keywords(sql, position, &["using"]);
            if let Some(schema) = schema {
                if let Some(tree) = &syntax_tree {
                    let referenced_tables = MysqlDialect::referenced_table_names_at_position(
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
        }

        crate::parser::CompletionContext::ReferenceColumnClause => {
            let prefix = common::cursor_prefix(sql, position);
            if let Some(schema) = schema {
                if let Some(table_reference) = SqlParser::reference_table_at_position(sql, position)
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
            for keyword in [
                "(",
                "MATCH FULL",
                "MATCH PARTIAL",
                "MATCH SIMPLE",
                "ON DELETE",
                "ON UPDATE",
            ] {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "0", keyword);
                items.push(item);
            }
        }

        crate::parser::CompletionContext::ReferenceRuleClause => {
            let prefix = common::cursor_prefix(sql, position);
            for keyword in ["CASCADE", "RESTRICT", "NO ACTION", "SET NULL"] {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "0", keyword);
                items.push(item);
            }
        }

        crate::parser::CompletionContext::ColumnTargetClause => {
            let prefix = common::cursor_prefix_excluding_keywords(sql, position, &["column"]);
            if let (Some(schema), Some(tree)) = (schema, &syntax_tree) {
                let referenced_tables = MysqlDialect::referenced_table_names_at_position(
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
            let prefix = common::cursor_prefix_excluding_keywords(sql, position, &["constraint"]);
            if let (Some(schema), Some(tree)) = (schema, &syntax_tree) {
                let referenced_tables = MysqlDialect::referenced_table_names_at_position(
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
                "CHANGE COLUMN",
                "RENAME COLUMN",
                "ADD CONSTRAINT",
                "DROP CONSTRAINT",
                "DROP FOREIGN KEY",
                "ADD INDEX",
                "DROP INDEX",
                "RENAME INDEX",
                "RENAME TO",
            ] {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "0", keyword);
                items.push(item);
            }
        }

        crate::parser::CompletionContext::InsertActionClause => {
            let prefix = common::cursor_prefix(sql, position);
            for keyword in ["(", "VALUES", "VALUE", "SELECT", "SET"] {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "0", keyword);
                items.push(item);
            }
        }

        crate::parser::CompletionContext::InsertValueClause => {
            let prefix = common::cursor_prefix(sql, position);
            for keyword in [
                "DEFAULT",
                "NULL",
                "TRUE",
                "FALSE",
                "CURRENT_DATE",
                "CURRENT_TIMESTAMP",
                "NOW()",
            ] {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "0", keyword);
                items.push(item);
            }
        }

        crate::parser::CompletionContext::InsertContinuationClause => {
            let prefix = common::cursor_prefix(sql, position);
            for keyword in ["ON DUPLICATE KEY UPDATE"] {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "0", keyword);
                items.push(item);
            }
        }

        crate::parser::CompletionContext::InsertConflictTargetClause
        | crate::parser::CompletionContext::InsertConflictConstraintClause
        | crate::parser::CompletionContext::InsertConflictActionClause => {}

        crate::parser::CompletionContext::ExpressionValueClause => {
            let prefix = common::cursor_prefix(sql, position);
            if let Some(schema) = schema {
                let referenced_tables = syntax_tree
                    .as_ref()
                    .map(|tree| {
                        MysqlDialect::referenced_table_names_at_position(
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
                    dialect.value_profile,
                );
            }
            let mut keywords = vec![
                "NULL",
                "TRUE",
                "FALSE",
                "CURRENT_DATE",
                "CURRENT_TIMESTAMP",
                "NOW()",
            ];
            if common::expression_value_allows_default(sql, position) {
                keywords.insert(0, "DEFAULT");
            }
            for keyword in keywords {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "0", keyword);
                items.push(item);
            }
        }

        crate::parser::CompletionContext::CaseResultClause => {
            let prefix = common::cursor_prefix_excluding_keywords(sql, position, &["then", "else"]);
            if let Some(schema) = schema {
                let referenced_tables = syntax_tree
                    .as_ref()
                    .map(|tree| {
                        MysqlDialect::referenced_table_names_at_position(
                            &parser, tree, sql, position, schema,
                        )
                    })
                    .unwrap_or_default();
                let use_table_prefix = referenced_tables.len() != 1;

                dialect.add_schema_columns(
                    &mut items,
                    schema,
                    &referenced_tables,
                    use_table_prefix,
                    &prefix,
                    "0",
                );
                dialect.add_schema_functions(
                    &mut items,
                    schema,
                    &prefix,
                    "1",
                    MysqlDialect::cursor_has_identifier_qualifier(sql, position),
                );
            }

            for keyword in [
                "NULL",
                "TRUE",
                "FALSE",
                "CURRENT_DATE",
                "CURRENT_TIMESTAMP",
                "NOW()",
            ] {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
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
                let mut item = dialect.create_keyword_item(keyword);
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
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "0", keyword);
                items.push(item);
            }
        }

        crate::parser::CompletionContext::PredicateContinuationClause => {
            let prefix = common::cursor_prefix(sql, position);
            for keyword in common::predicate_continuation_keywords(sql, position, false) {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "0", keyword);
                items.push(item);
            }
        }

        crate::parser::CompletionContext::UpdateActionClause => {
            let prefix = common::cursor_prefix(sql, position);
            for keyword in ["SET"] {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "0", keyword);
                items.push(item);
            }
        }

        crate::parser::CompletionContext::DeleteActionClause => {
            let prefix = common::cursor_prefix(sql, position);
            for keyword in ["WHERE", "ORDER BY", "LIMIT"] {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(keyword);
                common::set_completion_sort_text(&mut item, "0", keyword);
                items.push(item);
            }
        }

        crate::parser::CompletionContext::IndexTargetClause => {
            let prefix = common::cursor_prefix_excluding_keywords(sql, position, &["index"]);
            if let Some(schema) = schema {
                let referenced_tables = syntax_tree
                    .as_ref()
                    .map(|tree| {
                        MysqlDialect::referenced_table_names_at_position(
                            &parser, tree, sql, position, schema,
                        )
                    })
                    .unwrap_or_default();
                common::add_schema_indexes(
                    &mut items,
                    schema,
                    &referenced_tables,
                    &prefix,
                    "0",
                    MysqlDialect::cursor_has_identifier_qualifier(sql, position),
                );
            }
        }

        crate::parser::CompletionContext::DataTypeClause => {
            let prefix = common::cursor_prefix(sql, position);
            for data_type in [
                "VARCHAR",
                "TEXT",
                "INT",
                "BIGINT",
                "BOOLEAN",
                "DATETIME",
                "TIMESTAMP",
                "DATE",
                "DECIMAL",
                "JSON",
                "BLOB",
            ] {
                if !prefix.is_empty() && !data_type.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let mut item = dialect.create_keyword_item(data_type);
                common::set_completion_sort_text(&mut item, "0", data_type);
                items.push(item);
            }
        }

        crate::parser::CompletionContext::TableColumn => {
            // 表名.列名：只补全特定表的列名
            if let Some(tree) = &syntax_tree {
                if let Some(table_name) =
                    common::table_column_reference_at_position(&parser, tree, sql, position)
                {
                    if let Some(schema) = schema {
                        let aliases = snapshot.relation_aliases.clone().unwrap_or_else(|| {
                            SqlParser::relation_aliases_at_position(sql, position)
                        });

                        // Resolve aliases with SQL quoting/casing semantics.
                        let real_table_name = common::row_reference(schema, &aliases, &table_name);
                        if let Some(table) =
                            MysqlDialect::find_table_by_reference(schema, real_table_name)
                        {
                            for column in &table.columns {
                                items.push(dialect.create_column_item(column, None));
                            }
                        } else if SqlParser::resolve_relation_alias(&aliases, &table_name).is_none()
                        {
                            // An unresolved qualifier can name a schema function.
                            // Keep this existing SELECT capability while editing
                            // a qualified member prefix in the middle of a token.
                            dialect.add_schema_functions(
                                &mut items,
                                schema,
                                &common::cursor_prefix(sql, position),
                                "1",
                                true,
                            );
                        }
                    }
                }
            }
        }

        crate::parser::CompletionContext::Default => {
            let prefix = MysqlDialect::cursor_prefix(sql, position);

            let keywords = vec![
                "SELECT", "FROM", "WHERE", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER",
                "TABLE", "INDEX", "DATABASE", "SHOW", "DESCRIBE", "EXPLAIN", "JOIN", "INNER",
                "LEFT", "RIGHT", "OUTER", "ON", "GROUP", "BY", "ORDER", "HAVING", "LIMIT",
                "OFFSET", "UNION", "ALL", "DISTINCT", "AS", "AND", "OR", "NOT", "IN", "LIKE",
                "BETWEEN", "IS", "NULL", "TRUE", "FALSE",
            ];

            for keyword in keywords {
                if !prefix.is_empty() && !keyword.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                items.push(dialect.create_keyword_item(keyword));
            }

            if let Some(schema) = schema {
                common::add_schema_tables(
                    &mut items,
                    schema,
                    &prefix,
                    MysqlDialect::cursor_has_identifier_qualifier(sql, position),
                    MysqlDialect::cursor_has_identifier_qualifier(sql, position),
                );
                dialect.add_schema_functions(
                    &mut items,
                    schema,
                    &prefix,
                    "1",
                    MysqlDialect::cursor_has_identifier_qualifier(sql, position),
                );
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

    common::apply_value_completion_policy(&mut items, value_policy, dialect.value_profile);
    snapshot.items = items;
    snapshot
}
