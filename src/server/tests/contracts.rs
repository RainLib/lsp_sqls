use super::*;

#[test]
fn formatting_preferences_decode_editor_layout_values() {
    let preferences: FormattingPreferences = serde_json::from_value(serde_json::json!({
        "keywordCase": "lower",
        "functionCase": "upper",
        "logicalOperatorNewline": "none",
        "fromClauseLayout": "sameLine"
    }))
    .unwrap();

    assert!(matches!(preferences.keyword_case, KeywordCase::Lower));
    assert!(matches!(preferences.function_case, KeywordCase::Upper));
    assert_eq!(
        preferences.logical_operator_newline,
        LogicalOperatorNewline::None
    );
    assert_eq!(preferences.from_clause_layout, FromClauseLayout::SameLine);
}

#[test]
fn completion_preferences_default_function_case_preserves_legacy_clients() {
    let preferences: CompletionPreferences = serde_json::from_value(serde_json::json!({
        "keywordCase": "lower",
        "tableAlias": "none"
    }))
    .expect("completion preferences should deserialize");

    assert!(matches!(preferences.function_case, KeywordCase::Preserve));
}

#[test]
fn insert_value_hints_resolve_implicit_columns_from_metadata() {
    let sql = "INSERT INTO app.orders VALUES (1, 42, 9), (2, coalesce(7, 8), 10);";

    assert_eq!(
        inlay_labels(sql, &completion_schema_with_columns()),
        vec![
            "id:",
            "customer_id:",
            "total:",
            "id:",
            "customer_id:",
            "total:"
        ]
    );
}

#[test]
fn insert_value_hints_keep_explicit_quoted_columns_and_skip_database_owned_implicit_columns() {
    let mut schema = completion_schema_with_columns();
    schema.tables[0].columns[0].auto_increment = true;
    schema.tables[0].columns[2].generated = true;
    schema.tables[0].columns.push(Column {
        name: "system period".to_string(),
        hidden: true,
        ..Default::default()
    });
    let sql = concat!(
        "INSERT INTO app.orders VALUES (42);\n",
        "INSERT INTO [app].[orders] ([customer_id], \"display name\") ",
        "VALUES (7, 'Ada'), (8, json_object('name', 'Lin'));"
    );

    assert_eq!(
        inlay_labels(sql, &schema),
        vec![
            "customer_id:",
            "customer_id:",
            "display name:",
            "customer_id:",
            "display name:"
        ]
    );
}

#[test]
fn insert_value_hints_ignore_comments_and_strings() {
    let sql = concat!(
        "-- INSERT INTO app.orders VALUES (1, 2, 3);\n",
        "SELECT 'INSERT INTO app.orders VALUES (4, 5, 6)';"
    );

    assert!(inlay_labels(sql, &completion_schema_with_columns()).is_empty());
}

#[test]
fn insert_value_hints_map_insert_select_projection_to_explicit_columns() {
    let sql =
        "INSERT INTO app.orders (id, total) SELECT source_id, coalesce(total, 0) FROM staging;";

    assert_eq!(
        inlay_labels(sql, &completion_schema_with_columns()),
        vec!["id:", "total:"]
    );
}

#[test]
fn insert_value_hints_skip_select_modifiers_and_nested_commas() {
    let sql = concat!(
        "INSERT INTO app.orders (customer_id, total) ",
        "SELECT DISTINCT TOP (10) coalesce(primary_id, backup_id), count(*) ",
        "FROM staging;"
    );

    assert_eq!(
        inlay_labels(sql, &completion_schema_with_columns()),
        vec!["customer_id:", "total:"]
    );
}

#[test]
fn insert_value_hints_support_distinct_on_parenthesized_and_cte_select_sources() {
    let sql = concat!(
            "INSERT INTO app.orders (id, total) SELECT DISTINCT ON (tenant_id) source_id, total FROM staging;\n",
            "INSERT INTO app.orders (id, total) (SELECT source_id, total FROM staging);\n",
            "INSERT INTO app.orders (id, total) WITH staged AS (SELECT 1) SELECT source_id, total FROM staged;"
        );

    assert_eq!(
        inlay_labels(sql, &completion_schema_with_columns()),
        vec!["id:", "total:", "id:", "total:", "id:", "total:"]
    );
}

#[test]
fn insert_value_hints_resolve_implicit_insert_select_columns_from_metadata() {
    let mut schema = completion_schema_with_columns();
    schema.tables[0].columns[0].auto_increment = true;
    let sql = "INSERT INTO app.orders SELECT source_customer_id, source_total FROM staging;";

    assert_eq!(inlay_labels(sql, &schema), vec!["customer_id:", "total:"]);
}

#[test]
fn insert_value_hints_fail_closed_for_select_wildcards_and_arity_mismatch() {
    let schema = completion_schema_with_columns();
    for sql in [
        "INSERT INTO app.orders (id, total) SELECT * FROM staging;",
        "INSERT INTO app.orders (id, total) SELECT source.* FROM staging AS source;",
        "INSERT INTO app.orders (id, total) SELECT source_id FROM staging;",
        "INSERT INTO app.orders (id, total) SELECT source_id, total, extra FROM staging;",
    ] {
        assert!(inlay_labels(sql, &schema).is_empty(), "{sql}");
    }
}

#[test]
fn insert_value_hints_match_sql_server_catalog_and_implicit_dbo() {
    let mut schema = completion_schema_with_columns();
    schema.catalog = Some("AppDb".to_string());
    schema.database = "dbo".to_string();
    let sql = "INSERT INTO [AppDb]..[orders] WITH (TABLOCK) VALUES (1, 2, 3);";

    assert_eq!(
        inlay_labels(sql, &schema),
        vec!["id:", "customer_id:", "total:"]
    );
}
