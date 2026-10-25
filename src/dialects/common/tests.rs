use super::*;
use crate::parser::{RelationAlias, SqlParser};
use crate::schema::{Column, FunctionParameter, SchemaId};
use crate::schema::{Function, Schema, Table};

fn test_schema() -> Schema {
    Schema {
        id: SchemaId::new(),
        server: None,
        namespace: None,
        catalog: None,
        database: "app".to_string(),
        server_version: None,
        tables: vec![
            Table {
                name: "users".to_string(),
                columns: vec![
                    Column {
                        name: "id".to_string(),
                        data_type: "integer".to_string(),
                        nullable: false,
                        ..Default::default()
                    },
                    Column {
                        name: "name".to_string(),
                        data_type: "text".to_string(),
                        nullable: true,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            Table {
                name: "orders".to_string(),
                columns: vec![
                    Column {
                        name: "id".to_string(),
                        data_type: "integer".to_string(),
                        nullable: false,
                        ..Default::default()
                    },
                    Column {
                        name: "total".to_string(),
                        data_type: "numeric".to_string(),
                        nullable: false,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        ],
        functions: vec![Function {
            name: "calculate_score".to_string(),
            routine_type: Some("function".to_string()),
            parameters: vec![FunctionParameter {
                mode: None,
                name: "user_id".to_string(),
                data_type: "integer".to_string(),
                optional: false,
            }],
            return_type: "integer".to_string(),
            description: None,
        }],
        source_uri: None,
    }
}

#[test]
fn table_completion_respects_qualification_mode() {
    let schema = test_schema();
    let unqualified = create_table_item(&schema.tables[0], &schema, false);
    let qualified = create_table_item(&schema.tables[0], &schema, true);

    assert_eq!(unqualified.label, "users");
    assert_eq!(unqualified.insert_text.as_deref(), Some("users"));
    assert_eq!(unqualified.filter_text.as_deref(), Some("users"));
    assert_eq!(qualified.label, "app.users");
    assert_eq!(qualified.insert_text.as_deref(), Some("app.users"));
    assert_eq!(qualified.filter_text.as_deref(), Some("users"));
}

#[test]
fn column_completion_marks_database_owned_visibility_without_hiding_the_item() {
    let hidden = create_column_item(
        &Column {
            name: "period start".to_string(),
            data_type: "timestamp".to_string(),
            hidden: true,
            ..Default::default()
        },
        Some("events"),
    );
    assert_eq!(hidden.label, "events.period start");
    assert!(hidden
        .detail
        .as_deref()
        .is_some_and(|detail| detail.ends_with("· hidden")));
    assert!(hidden
        .sort_text
        .as_deref()
        .is_some_and(|sort| sort.starts_with("3:")));

    let generated = create_column_item(
        &Column {
            name: "search_vector".to_string(),
            data_type: "tsvector".to_string(),
            generated: true,
            ..Default::default()
        },
        None,
    );
    assert!(generated
        .detail
        .as_deref()
        .is_some_and(|detail| detail.ends_with("· generated")));

    let mut schema = test_schema();
    schema.tables[0].columns.push(Column {
        name: "period_start".to_string(),
        data_type: "timestamp".to_string(),
        hidden: true,
        ..Default::default()
    });
    let mut items = Vec::new();
    add_schema_columns(&mut items, &schema, &["users".to_string()], false, "", "0");
    let visible = items.iter().find(|item| item.label == "name").unwrap();
    let hidden = items
        .iter()
        .find(|item| item.label == "period_start")
        .unwrap();
    assert!(hidden
        .detail
        .as_deref()
        .is_some_and(|detail| detail.ends_with("· hidden")));
    assert!(visible.sort_text < hidden.sort_text);
}

#[test]
fn schema_columns_filter_referenced_tables_and_prefixes() {
    let schema = test_schema();
    let mut items = Vec::new();
    add_schema_columns(
        &mut items,
        &schema,
        &["app.users".to_string()],
        false,
        "na",
        "0",
    );

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "name");
    assert_eq!(items[0].filter_text.as_deref(), Some("name"));

    let mut prefixed_items = Vec::new();
    add_schema_columns(
        &mut prefixed_items,
        &schema,
        &["users".to_string(), "orders".to_string()],
        true,
        "",
        "0",
    );
    let labels = prefixed_items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        labels,
        vec!["users.id", "users.name", "orders.id", "orders.total"]
    );
    assert_eq!(
        prefixed_items
            .iter()
            .map(|item| item.insert_text.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("users.id"),
            Some("users.name"),
            Some("orders.id"),
            Some("orders.total")
        ]
    );
}

#[test]
fn schema_columns_disambiguate_duplicate_names_before_from() {
    let schema = test_schema();
    let mut items = Vec::new();
    add_schema_columns(&mut items, &schema, &[], false, "id", "0");

    assert_eq!(
        items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        vec!["users.id", "orders.id"]
    );
    assert!(items
        .iter()
        .all(|item| item.insert_text.as_deref() == Some(item.label.as_str())));
    assert_ne!(items[0].sort_text, items[1].sort_text);
    assert!(items.iter().all(|item| item.data.is_some()));
}

#[test]
fn schema_columns_rank_abbreviation_and_fuzzy_matches_after_prefixes() {
    assert_eq!(identifier_match_rank("user_id", "ui"), Some(2));
    assert_eq!(identifier_match_rank("created_at", "cat"), Some(3));
    assert_eq!(identifier_match_rank("ÄlphaOrders", "äl"), Some(1));
    assert_eq!(identifier_match_rank("用户订单", "户单"), Some(3));
    assert_eq!(identifier_match_rank("account_id", "zzz"), None);
}

#[test]
fn aliased_join_columns_use_aliases_for_label_and_insertion() {
    let schema = test_schema();
    let mut items = Vec::new();
    add_schema_columns(
        &mut items,
        &schema,
        &["users".to_string(), "orders".to_string()],
        true,
        "id",
        "0",
    );
    let aliases = vec![
        RelationAlias {
            name: "u".to_string(),
            sql: "u".to_string(),
            relation: "users".to_string(),
        },
        RelationAlias {
            name: "o".to_string(),
            sql: "o".to_string(),
            relation: "orders".to_string(),
        },
    ];
    apply_column_aliases(&mut items, &schema, &aliases);

    assert_eq!(
        items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        vec!["u.id", "o.id"]
    );
    assert_eq!(
        items
            .iter()
            .map(|item| item.insert_text.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("u.id"), Some("o.id")]
    );
    assert!(items
        .iter()
        .all(|item| item.filter_text.as_deref() == Some("id")));
}

#[test]
fn aliased_columns_preserve_quoted_sql_spelling() {
    let schema = test_schema();
    let mut items = Vec::new();
    add_schema_columns(&mut items, &schema, &["users".to_string()], true, "id", "0");
    let aliases = vec![RelationAlias {
        name: "User Alias".to_string(),
        sql: "\"User Alias\"".to_string(),
        relation: "users".to_string(),
    }];
    apply_column_aliases(&mut items, &schema, &aliases);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "User Alias.id");
    assert_eq!(items[0].insert_text.as_deref(), Some("\"User Alias\".id"));
    assert_eq!(items[0].filter_text.as_deref(), Some("id"));
}

#[test]
fn functions_match_schema_qualified_references() {
    let schema = test_schema();
    let function = find_function_by_reference(&schema, "app.calculate_score(")
        .expect("schema-qualified function reference should match");

    assert_eq!(function.name, "calculate_score");
    let item = create_function_item(function, &schema, true);
    assert_eq!(item.label, "app.calculate_score");
    assert_eq!(item.filter_text.as_deref(), Some("calculate_score"));
}

#[test]
fn referenced_table_names_deduplicate_aliases_preserving_order() {
    let sql = "SELECT * FROM app.users u JOIN app.orders o ON o.user_id = u.id JOIN app.users managers ON managers.id = u.manager_id";
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    let tree = result.tree.as_ref().expect("SQL should parse");

    assert_eq!(
        referenced_table_names(&parser, tree, sql),
        vec!["app.users".to_string(), "app.orders".to_string()]
    );
}

#[test]
fn compact_formatting_preserves_literals_comments_and_dollar_quotes() {
    let sql = "  SELECT   'a   b',  \"spaced  name\"  -- keep   comment\n  FROM   users  WHERE body = $tag$line  one\nline   two$tag$  /* block   comment */  ";

    assert_eq!(
            compact_sql_whitespace(sql),
            "SELECT 'a   b', \"spaced  name\" -- keep   comment\nFROM users WHERE body = $tag$line  one\nline   two$tag$ /* block   comment */"
        );
}

#[test]
fn compact_formatting_preserves_escaped_and_bracketed_content() {
    assert_eq!(
        compact_sql_whitespace("SELECT  'it''s  spaced',  `my  column`,  [also  spaced]  FROM  t"),
        "SELECT 'it''s  spaced', `my  column`, [also  spaced] FROM t"
    );
}

#[test]
fn formatter_keyword_case_supports_upper_lower_and_preserve() {
    let source = "seLEct value frOM users whERE active = true";

    assert!(format_sql_pretty_with_keyword_case(source, Some(true)).contains("SELECT"));
    assert!(format_sql_pretty_with_keyword_case(source, Some(false)).contains("select"));
    let preserved = format_sql_pretty_with_keyword_case(source, None);
    assert!(preserved.contains("seLEct"), "formatted SQL: {preserved}");
    assert!(preserved.contains("frOM"), "formatted SQL: {preserved}");
}

#[test]
fn formatter_function_case_rewrites_only_invocation_names() {
    let source = "SELECT public.MixedFn(${value}), \"QuotedFn\"(), CAST(score AS DECIMAL(10, 2)) FROM AppTable WHERE id IN (SELECT id FROM OtherTable)";
    let lower = apply_formatter_function_case(source, Some(false));

    assert!(lower.contains("public.mixedfn(${value})"));
    assert!(lower.contains("\"QuotedFn\"()"));
    assert!(lower.contains("DECIMAL(10, 2)"));
    assert!(lower.contains("FROM AppTable"));
    assert!(lower.contains("FROM OtherTable"));
    assert!(lower.contains("${value}"));
    assert_eq!(apply_formatter_function_case(source, None), source);
}

#[test]
fn formatter_layout_keeps_logical_operators_on_one_line_without_touching_noise() {
    let formatted = "SELECT *\nFROM users\nWHERE note = 'AND OR'\n  AND active = true\n  OR role = \"OR\"\n-- AND comment\n  AND deleted_at IS NULL";

    assert_eq!(
            apply_formatter_layout(formatted, LogicalOperatorLayout::SameLine, false),
            "SELECT *\nFROM users\nWHERE note = 'AND OR' AND active = true OR role = \"OR\"\n-- AND comment\n  AND deleted_at IS NULL"
        );
}

#[test]
fn formatter_layout_moves_logical_operators_after_the_previous_expression() {
    let formatted =
        "SELECT *\nFROM users\nWHERE active = true\n  AND role = 'admin'\n  OR role = 'owner'";

    assert_eq!(
        apply_formatter_layout(formatted, LogicalOperatorLayout::After, false),
        "SELECT *\nFROM users\nWHERE active = true AND\n  role = 'admin' OR\n  role = 'owner'"
    );
}

#[test]
fn formatter_layout_keeps_plain_from_sources_inline_but_not_nested_sources() {
    assert_eq!(
        apply_formatter_layout(
            "SELECT *\nFROM\n  users\nWHERE id = 1",
            LogicalOperatorLayout::Before,
            true,
        ),
        "SELECT *\nFROM users\nWHERE id = 1"
    );
    assert_eq!(
        apply_formatter_layout(
            "SELECT *\nFROM\n  (SELECT id FROM users) nested",
            LogicalOperatorLayout::Before,
            true,
        ),
        "SELECT *\nFROM\n  (SELECT id FROM users) nested"
    );
}

#[test]
fn parser_active_statement_start_recovers_a_second_query_without_a_semicolon() {
    let sql = "SELECT * FROM first_table\n\nSELECT * FROM second_table WHERE ";
    let start = SqlParser::active_statement_start(sql);

    assert_eq!(&sql[start..], "SELECT * FROM second_table WHERE ");
}

#[test]
fn parser_active_statement_start_preserves_ctes_set_operations_and_sql_noise() {
    for sql in [
        "WITH recent AS (\n  SELECT 1\n)\n\nSELECT * FROM recent",
        "SELECT 1\nUNION ALL\n\nSELECT 2",
        "SELECT '-- SELECT'\n\n/* SELECT */\nFROM logs",
    ] {
        assert_eq!(
            SqlParser::active_statement_start(sql),
            0,
            "should not split: {sql}"
        );
    }
}

#[test]
fn foreign_key_templates_do_not_treat_cte_projection_as_catalog_constraints() {
    let mut schema = test_schema();
    let orders = schema
        .tables
        .iter_mut()
        .find(|table| table.name == "orders")
        .unwrap();
    orders.columns.push(Column {
        name: "user_id".into(),
        data_type: "integer".into(),
        ..Default::default()
    });
    orders.constraints = vec![crate::schema::Constraint {
        name: "orders_user".into(),
        constraint_type: "FOREIGN KEY".into(),
        columns: vec!["user_id".into()],
        referenced_table: Some("users".into()),
        referenced_columns: vec!["id".into()],
        ..Default::default()
    }];
    crate::relation_bindings::install(
        &mut schema,
        Table {
            name: "users".into(),
            object_type: Some("COMMON TABLE EXPRESSION".into()),
            columns: vec![Column {
                name: "different_key".into(),
                data_type: "text".into(),
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    let mut items = Vec::new();
    add_foreign_key_join_snippets(
        &mut items,
        &schema,
        &["users".into()],
        &std::collections::HashMap::new(),
        "",
        false,
    );
    assert!(items.is_empty(), "{items:?}");
    add_foreign_key_join_snippets(
        &mut items,
        &schema,
        &["app.users".into()],
        &std::collections::HashMap::new(),
        "",
        false,
    );
    assert!(
        !items.is_empty(),
        "qualified physical FK metadata remains available"
    );
    items.clear();
    add_foreign_key_join_snippets(
        &mut items,
        &schema,
        &["orders".into()],
        &std::collections::HashMap::new(),
        "",
        false,
    );
    assert!(
        items.iter().all(|item| item
            .insert_text
            .as_ref()
            .is_some_and(|text| text.starts_with("app.users "))),
        "{items:?}"
    );
    assert!(!items.is_empty());
}
