use super::*;

#[test]
fn local_relation_augmentation_extracts_dbx_cte_and_derived_projection_shapes() {
    let sql = "WITH recent_users(id, display_name) AS (\
            SELECT id, concat(name, ',FROM') AS display_name FROM users\
        ) SELECT * FROM recent_users ru JOIN (\
            SELECT id, name AS user_name FROM users\
        ) sq ON sq.id = ru.id WHERE sq.";
    let augmented = augment_schema_with_local_relations(
        semantic_test_schema(),
        sql,
        lsp_position_at_end(sql),
        "oxide://query/semantic.postgres.sql",
        Some("postgres"),
    );

    let recent = augmented
        .tables
        .iter()
        .find(|table| table.name == "recent_users")
        .expect("CTE relation");
    assert_eq!(
        recent
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "display_name"]
    );
    assert_eq!(
        recent.object_type.as_deref(),
        Some("COMMON TABLE EXPRESSION")
    );

    let derived = augmented
        .tables
        .iter()
        .find(|table| table.name == "sq")
        .expect("derived relation");
    assert_eq!(
        derived
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "user_name"]
    );
    assert_eq!(derived.object_type.as_deref(), Some("DERIVED TABLE"));
}

#[test]
fn local_relation_scan_ignores_keywords_inside_comments_and_literals() {
    let sql = "-- WITH ghost(id) AS (SELECT id FROM users)\n\
            SELECT 'FROM (SELECT id FROM users) fake' AS note FROM users";
    let augmented = augment_schema_with_local_relations(
        semantic_test_schema(),
        sql,
        lsp_position_at_end(sql),
        "oxide://query/noise.postgres.sql",
        Some("postgres"),
    );

    assert!(!augmented.tables.iter().any(|table| table.name == "ghost"));
    assert!(!augmented.tables.iter().any(|table| table.name == "fake"));
}

#[test]
fn local_relation_scan_is_bounded_for_large_console_documents() {
    let mut sql = " ".repeat(LOCAL_RELATION_SCAN_MAX_BYTES + 1);
    sql.push_str("WITH recent(id) AS (SELECT id FROM users) SELECT * FROM recent");
    let augmented = augment_schema_with_local_relations(
        semantic_test_schema(),
        &sql,
        lsp_position_at_end(&sql),
        "oxide://query/large.postgres.sql",
        Some("postgres"),
    );

    assert!(!augmented.tables.iter().any(|table| table.name == "recent"));
}

#[test]
fn local_relation_augmentation_models_dbx_correlation_column_shapes() {
    let cases = [
            (
                "SELECT * FROM generate_series(1, 3) g(value) WHERE g.",
                "generate_series",
                vec!["value"],
                "TABLE FUNCTION",
            ),
            (
                "SELECT * FROM generate_series(1, 3) WITH ORDINALITY AS g(value, ord), users u WHERE g.",
                "generate_series",
                vec!["value", "ord"],
                "TABLE FUNCTION",
            ),
            (
                "SELECT * FROM users u(user_id) WHERE u.",
                "u",
                vec!["user_id", "name", "email"],
                "CORRELATED TABLE",
            ),
            (
                "SELECT * FROM (SELECT id, name FROM users) u(user_id, display_name) WHERE u.",
                "u",
                vec!["user_id", "display_name"],
                "DERIVED TABLE",
            ),
            (
                "SELECT * FROM users u, LATERAL (SELECT u.id AS user_id) s WHERE s.",
                "s",
                vec!["user_id"],
                "DERIVED TABLE",
            ),
            (
                "SELECT * FROM users u CROSS APPLY (SELECT u.id AS user_id) s WHERE s.",
                "s",
                vec!["user_id"],
                "DERIVED TABLE",
            ),
            (
                "SELECT * FROM users u, LATERAL generate_series(1, 3) g(value), users o WHERE g.",
                "generate_series",
                vec!["value"],
                "TABLE FUNCTION",
            ),
        ];

    for (sql, table_name, expected_columns, object_type) in cases {
        let augmented = augment_schema_with_local_relations(
            semantic_test_schema(),
            sql,
            lsp_position_at_end(sql),
            "oxide://query/correlation.postgres.sql",
            Some("postgres"),
        );
        let table = augmented
            .tables
            .iter()
            .find(|table| table.name == table_name)
            .unwrap_or_else(|| panic!("missing {table_name}: {:?}", augmented.tables));
        assert_eq!(
            table
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            expected_columns
        );
        assert_eq!(table.object_type.as_deref(), Some(object_type));
    }
}

#[test]
fn sqlserver_table_hints_preserve_remote_column_shapes() {
    for sql in [
        "SELECT * FROM users u (NOLOCK) WHERE u.",
        "SELECT * FROM users u WITH (UPDLOCK, ROWLOCK) WHERE u.",
    ] {
        let augmented = augment_schema_with_local_relations(
            semantic_test_schema(),
            sql,
            lsp_position_at_end(sql),
            "oxide://query/hints.sqlserver.sql",
            Some("postgres"),
        );
        let users = augmented
            .tables
            .iter()
            .find(|table| table.name == "users")
            .expect("remote users table");
        assert_eq!(
            users
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            ["id", "name", "email"]
        );
        assert_ne!(users.object_type.as_deref(), Some("CORRELATED TABLE"));
    }
}

#[tokio::test]
async fn dbx_cte_column_scope_is_shared_by_native_and_compatibility_dialects() {
    let sql = "WITH recent_users(id, display_name) AS (SELECT id, name FROM users) \
                   SELECT * FROM recent_users ru WHERE ru.";
    let position = lsp_position_at_end(sql);
    let augmented = augment_schema_with_local_relations(
        semantic_test_schema(),
        sql,
        position,
        "oxide://query/cte.postgres.sql",
        Some("postgres"),
    );
    let registry = DialectRegistry::new();

    for dialect_name in [
        "postgres",
        "mysql",
        "sqlite",
        "hive",
        "clickhouse",
        "oracle",
        "sqlserver",
        "duckdb",
    ] {
        let dialect = registry
            .get_by_name(dialect_name)
            .unwrap_or_else(|| panic!("registered dialect {dialect_name}"));
        let items = dialect.completion(sql, position, Some(&augmented)).await;
        let labels = items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();
        assert!(
            labels.contains(&"id") && labels.contains(&"display_name"),
            "{dialect_name} should expose CTE projection columns: {labels:?}"
        );
        assert!(
            !labels.contains(&"name"),
            "{dialect_name} should not leak base-table columns into alias scope: {labels:?}"
        );
    }
}
