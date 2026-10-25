use super::*;

async fn typed(dialect: &str, sql: &str, name: &str, ty: &str) {
    let harness = configured(dialect, sql);
    let items = complete_harness(&harness).await;
    let item = items
        .iter()
        .find(|item| item.label == name)
        .unwrap_or_else(|| panic!("{dialect}: {sql}: {items:?}"));
    assert!(
        item.detail
            .as_ref()
            .is_some_and(|detail| detail.contains(ty)),
        "{dialect}: {sql}: {item:?}"
    );
    let hover = harness
        .service
        .inner()
        .hover(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .expect("namespace hover");
    assert!(
        serde_json::to_string(&hover).unwrap().contains(ty),
        "{dialect}: {sql}: {hover:?}"
    );
}

#[tokio::test]
async fn cte_and_qualified_physical_homonym_keep_distinct_column_types() {
    for dialect in DIALECTS.iter().copied().chain(["sqlserver", "oracle"]) {
        for (sql,ty) in [
            ("WITH users(name) AS (SELECT u.id FROM app.users u) SELECT p.na<cursor>me FROM app.users p JOIN users c ON p.id=c.name","text"),
            ("WITH users(name) AS (SELECT u.id FROM app.users u) SELECT c.na<cursor>me FROM app.users p JOIN users c ON p.id=c.name","bigint"),
        ] { typed(dialect,sql,"name",ty).await; }
    }
}

#[tokio::test]
async fn derived_alias_does_not_replace_schema_qualified_base_table() {
    for dialect in DIALECTS.iter().copied() {
        typed(dialect,"SELECT p.na<cursor>me FROM app.users p JOIN (SELECT u.id AS name FROM app.users u) users ON users.name=p.id","name","text").await;
        typed(dialect,"SELECT users.na<cursor>me FROM app.users p JOIN (SELECT u.id AS name FROM app.users u) users ON users.name=p.id","name","bigint").await;
    }
}

#[tokio::test]
async fn unqualified_local_columns_do_not_leak_shadowed_physical_fields() {
    for dialect in DIALECTS.iter().copied() {
        let items = candidates(
            dialect,
            "WITH users(local_id) AS (SELECT id FROM app.users) SELECT <cursor> FROM users",
        )
        .await;
        assert!(
            items
                .iter()
                .any(|item| item.label == "local_id" || item.label == "users.local_id"),
            "{dialect}: {items:?}"
        );
        assert!(
            !items.iter().any(|item| matches!(
                item.label.as_str(),
                "name" | "email" | "users.name" | "users.email"
            )),
            "{dialect}: {items:?}"
        );
    }
}

#[tokio::test]
async fn mixed_star_and_named_projections_preserve_both_namespaces() {
    typed("postgres","WITH users(local_name) AS (SELECT id FROM app.users), combined(physical_name,local_key) AS (SELECT p.name,c.local_name FROM app.users p JOIN users c ON p.id=c.local_name) SELECT x.phys<cursor>ical_name FROM combined x","physical_name","text").await;
    typed("postgres","WITH users(local_name) AS (SELECT id FROM app.users), combined AS (SELECT p.*,c.* FROM app.users p JOIN users c ON p.id=c.local_name) SELECT x.local<cursor>_name FROM combined x","local_name","bigint").await;
}

#[tokio::test]
async fn relation_candidates_insert_qualified_catalog_and_bare_cte_names() {
    let items = candidates(
        "postgres",
        "WITH users(local_id) AS (SELECT id FROM app.users) SELECT * FROM us<cursor>",
    )
    .await;
    assert!(items.iter().any(|item| item.label == "users"));
    assert!(
        items.iter().any(|item| item.label == "app.users"),
        "{items:?}"
    );
    let items = candidates(
        "postgres",
        "WITH users(local_id) AS (SELECT id FROM app.users) SELECT * FROM app.us<cursor>",
    )
    .await;
    let tables: Vec<_> = items
        .iter()
        .filter(|item| item.label.ends_with("users"))
        .collect();
    assert_eq!(tables.len(), 1, "{items:?}");
    assert!(
        tables[0]
            .documentation
            .as_ref()
            .is_some_and(|doc| !serde_json::to_string(doc).unwrap().contains("local_id")),
        "{:?}",
        tables[0]
    );
}

#[tokio::test]
async fn mixed_namespace_candidates_keep_their_own_aliases() {
    for dialect in ["postgres", "mysql", "sqlite", "hive", "clickhouse"] {
        let items=candidates(dialect,"WITH users(name) AS (SELECT id FROM app.users) SELECT na<cursor> FROM app.users p JOIN users c ON p.id=c.name").await;
        for (name, ty) in [("p.name", "text"), ("c.name", "bigint")] {
            let item = items
                .iter()
                .find(|item| item.label == name)
                .unwrap_or_else(|| panic!("{dialect}: {name}: {items:?}"));
            assert!(
                item.detail
                    .as_ref()
                    .is_some_and(|detail| detail.contains(ty)),
                "{item:?}"
            );
        }
        assert!(!items.iter().any(|item| item.label == "users.name"));
    }
}

#[tokio::test]
async fn qualified_reference_never_binds_to_a_query_only_name() {
    for dialect in DIALECTS.iter().copied() {
        let items=candidates(dialect,"WITH projected(local_id) AS (SELECT id FROM users) SELECT q.<cursor> FROM app.projected q").await;
        assert!(
            !items.iter().any(|item| item.label == "local_id"),
            "{dialect}: {items:?}"
        );
        typed(dialect,"WITH users(name) AS (SELECT id FROM app.users) SELECT app.users.na<cursor>me FROM app.users JOIN users c ON app.users.id=c.name","name","text").await;
    }
}

#[tokio::test]
async fn recursive_homonym_keeps_qualified_physical_source_in_member() {
    for (dialect, keyword) in [
        ("postgres", "RECURSIVE"),
        ("mysql", "RECURSIVE"),
        ("sqlserver", ""),
    ] {
        typed(dialect,&format!("WITH {keyword} users(local_id,name) AS (SELECT id,id FROM app.users WHERE id=1 UNION ALL SELECT p.id,p.id FROM app.users p JOIN users c ON p.id=c.local_id+1 WHERE c.local_id<25 AND p.na<cursor>me IS NOT NULL) SELECT * FROM users"),"name","text").await;
    }
}

#[tokio::test]
async fn qualified_base_and_cte_types_refresh_without_mutating_catalog() {
    let harness=configured("postgres","WITH users(name) AS (SELECT id FROM app.users) SELECT p.na<cursor>me FROM app.users p JOIN users c ON p.id=c.name");
    let server = harness.service.inner();
    let id = *server
        .configured_file_schemas
        .get(harness.uri.as_str())
        .unwrap();
    for expected in ["text", "varchar(40)", "text"] {
        let mut schema = server.schema_manager.get(id).unwrap();
        let table = schema
            .tables
            .iter_mut()
            .find(|table| table.name == "users")
            .unwrap();
        table
            .columns
            .iter_mut()
            .find(|column| column.name == "name")
            .unwrap()
            .data_type = expected.into();
        assert!(server.schema_manager.update(id, schema));
        let items = complete_harness(&harness).await;
        let item = items.iter().find(|item| item.label == "name").unwrap();
        assert!(
            item.detail
                .as_ref()
                .is_some_and(|detail| detail.contains(expected)),
            "{item:?}"
        );
        let stored = server.schema_manager.get(id).unwrap();
        assert_eq!(stored.tables.len(), 3);
        assert!(stored
            .tables
            .iter()
            .all(|table| !crate::relation_bindings::query_local(table)));
    }
}

#[tokio::test]
async fn generic_sql_hover_does_not_match_table_names_in_literals_or_other_scopes() {
    for dialect in ["hive", "clickhouse"] {
        for marked in [
            "SELECT 'us<cursor>ers' FROM users u",
            "SELECT u.id FROM users u /* us<cursor>ers */",
        ] {
            let harness = configured(dialect, marked);
            let hover=harness.service.inner().hover(serde_json::from_value(serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position})).unwrap()).await.unwrap();
            assert!(hover.is_none(), "{dialect}: {marked}: {hover:?}");
        }
    }
}

#[tokio::test]
async fn correlation_column_aliases_are_bound_to_the_invocation() {
    typed("postgres","SELECT u.dis<cursor>play_name FROM app.users AS u(user_key,display_name,mail) JOIN app.users p ON p.id=u.user_key","display_name","text").await;
    typed("postgres","SELECT p.na<cursor>me FROM app.users AS u(user_key,display_name,mail) JOIN app.users p ON p.id=u.user_key","name","text").await;
    typed("postgres","WITH selected AS (SELECT u.* FROM app.users AS u(user_key,display_name,mail)) SELECT s.dis<cursor>play_name FROM selected s","display_name","text").await;
    let items = candidates(
        "postgres",
        "SELECT disp<cursor> FROM app.users AS u(user_key,display_name,mail)",
    )
    .await;
    assert!(
        items
            .iter()
            .any(|item| item.label == "display_name" || item.label == "u.display_name"),
        "{items:?}"
    );
}

#[tokio::test]
async fn unqualified_table_function_candidates_keep_each_invocation_shape() {
    let items=candidates("mysql","SELECT <cursor> FROM users u JOIN JSON_TABLE(u.payload,'$' COLUMNS(alpha INT PATH '$.a')) a ON TRUE JOIN JSON_TABLE(u.payload,'$' COLUMNS(beta TEXT PATH '$.b')) b ON TRUE").await;
    assert!(
        items.iter().any(|item| item.label == "a.alpha"),
        "{items:?}"
    );
    assert!(items.iter().any(|item| item.label == "b.beta"), "{items:?}");
    assert!(
        !items
            .iter()
            .any(|item| matches!(item.label.as_str(), "a.beta" | "b.alpha")),
        "{items:?}"
    );
}

#[tokio::test]
async fn table_hover_preserves_schema_qualification_over_cte_shadowing() {
    for dialect in DIALECTS.iter().copied() {
        for (reference, expected, forbidden) in [
            ("app.us<cursor>ers", "email", "local_id"),
            ("us<cursor>ers", "local_id", "email"),
        ] {
            let sql = format!(
                "WITH users(local_id) AS (SELECT id FROM app.users) SELECT * FROM {reference}"
            );
            let harness = configured(dialect, &sql);
            let hover = harness
                .service
                .inner()
                .hover(
                    serde_json::from_value(serde_json::json!({
                        "textDocument":{"uri":harness.uri},"position":harness.position
                    }))
                    .unwrap(),
                )
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{dialect}: {sql}"));
            let text = serde_json::to_string(&hover).unwrap();
            assert!(text.contains(expected), "{dialect}: {sql}: {text}");
            assert!(!text.contains(forbidden), "{dialect}: {sql}: {text}");
        }
    }
}
