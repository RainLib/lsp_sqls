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
        .expect("recursive hover");
    assert!(
        serde_json::to_string(&hover).unwrap().contains(ty),
        "{dialect}: {hover:?}"
    );
}

#[tokio::test]
async fn recursive_cte_anchor_types_reach_outer_and_recursive_member() {
    for (dialect, keyword, anchor, ty) in [
        ("postgres", "RECURSIVE", "0", "integer"),
        ("mysql", "RECURSIVE", "CAST(0 AS SIGNED)", "SIGNED"),
        ("sqlserver", "", "CAST(0 AS int)", "int"),
    ] {
        for member in [true, false] {
            let expression = if member {
                "w.de<cursor>pth+1"
            } else {
                "w.depth+1"
            };
            let outer = if member { "w.depth" } else { "w.de<cursor>pth" };
            typed(dialect,&format!("WITH {keyword} walk(user_key,depth) AS (SELECT u.id,{anchor} FROM users u UNION ALL SELECT w.user_key,{expression} FROM walk w WHERE w.depth<10) SELECT {outer} FROM walk w"),"depth",ty).await;
        }
    }
}

#[tokio::test]
async fn recursive_cte_infers_anchor_names_and_flows_to_following_cte() {
    for dialect in ["postgres", "mysql"] {
        let sql="WITH RECURSIVE walk AS (SELECT u.id AS user_key,CAST(0 AS bigint) AS depth FROM users u UNION ALL SELECT w.user_key,w.depth+1 FROM walk w WHERE w.depth<10), ranked AS (SELECT w.* FROM walk w) SELECT r.us<cursor>er_key FROM ranked r";
        typed(
            dialect,
            &sql.replace(
                "CAST(0 AS bigint)",
                if dialect == "mysql" {
                    "CAST(0 AS SIGNED)"
                } else {
                    "CAST(0 AS bigint)"
                },
            ),
            "user_key",
            "bigint",
        )
        .await;
    }
}

#[tokio::test]
async fn recursive_self_reference_shadows_physical_homonym() {
    let items=candidates("postgres","WITH RECURSIVE users(item_key,depth) AS (SELECT 1,0 UNION ALL SELECT u.<cursor> FROM users u WHERE u.depth<10) SELECT * FROM users").await;
    assert!(items.iter().any(|item| item.label == "item_key"));
    assert!(items.iter().any(|item| item.label == "depth"));
    assert!(
        !items
            .iter()
            .any(|item| matches!(item.label.as_str(), "name" | "email" | "id")),
        "{items:?}"
    );
}

#[tokio::test]
async fn recursive_anchor_scope_does_not_expose_later_or_sibling_ctes() {
    let items=candidates("postgres","WITH RECURSIVE walk(user_key,depth) AS (SELECT u.<cursor>,0 FROM users u UNION ALL SELECT w.user_key,w.depth+1 FROM walk w), later(secret) AS (SELECT audit_secret FROM audit_events) SELECT * FROM walk").await;
    assert!(items.iter().any(|item| item.label == "id"));
    assert!(!items
        .iter()
        .any(|item| matches!(item.label.as_str(), "depth" | "secret" | "audit_secret")));
}

#[tokio::test]
async fn nonrecursive_union_still_uses_set_type_resolution() {
    typed("postgres","WITH RECURSIVE numbers AS (SELECT 1::integer AS value UNION ALL SELECT 2::bigint) SELECT n.va<cursor>lue FROM numbers n","value","bigint").await;
}

#[tokio::test]
async fn recursive_path_types_survive_arrays_joins_windows_and_derived_projection() {
    let sql="WITH RECURSIVE walk(user_key,depth,path) AS (SELECT u.id,0,ARRAY[u.id]::bigint[] FROM users u WHERE u.id=1 UNION ALL SELECT u.id,w.depth+1,w.path||u.id FROM users u JOIN walk w ON u.id=w.user_key+1 WHERE w.depth<10), summarized AS (SELECT w.*, row_number() OVER (ORDER BY depth DESC) AS position FROM walk w) SELECT r.pa<cursor>th FROM (SELECT * FROM summarized WHERE position<=50) r";
    typed("postgres", sql, "path", "bigint[]").await;
}

#[tokio::test]
async fn recursive_anchor_types_refresh_when_live_metadata_changes() {
    let harness=configured("postgres","WITH RECURSIVE walk(user_key) AS (SELECT u.id FROM users u UNION ALL SELECT w.user_key FROM walk w WHERE w.user_key IS NOT NULL) SELECT w.us<cursor>er_key FROM walk w");
    let server = harness.service.inner();
    let id = *server
        .configured_file_schemas
        .get(harness.uri.as_str())
        .unwrap();
    for expected in ["bigint", "text", "integer"] {
        let mut schema = server.schema_manager.get(id).unwrap();
        schema.tables[0]
            .columns
            .iter_mut()
            .find(|column| column.name == "id")
            .unwrap()
            .data_type = expected.into();
        assert!(server.schema_manager.update(id, schema));
        let items = complete_harness(&harness).await;
        let item = items.iter().find(|item| item.label == "user_key").unwrap();
        assert!(
            item.detail
                .as_ref()
                .is_some_and(|detail| detail.contains(expected)),
            "{item:?}"
        );
    }
}
