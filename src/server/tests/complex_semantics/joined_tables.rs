use super::*;
use tower_lsp::lsp_types::{HoverParams, NumberOrString};
async fn typed(dialect: &str, sql: &str, name: &str, kind: &str) {
    let h = configured(dialect, sql);
    let items = complete_harness(&h).await;
    assert!(
        items
            .iter()
            .any(|i| i.label == name && i.detail.as_deref().is_some_and(|d| d.contains(kind))),
        "{dialect} {sql}: {items:?}"
    );
    let params: HoverParams = serde_json::from_value(
        serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
    )
    .unwrap();
    let hover = h.service.inner().hover(params).await.unwrap();
    assert!(
        serde_json::to_string(&hover).unwrap().contains(kind),
        "{sql}: {hover:?}"
    );
}
#[tokio::test]
async fn joined_tables_unaliased_group_members_keep_types() {
    for dialect in DIALECTS.iter().copied().chain(["sqlserver"]) {
        typed(
            dialect,
            "SELECT u.na<cursor>me FROM (users u JOIN orders o ON u.id=o.customer_id);",
            "name",
            "text",
        )
        .await;
        typed(dialect,"SELECT o.to<cursor>tal FROM users u JOIN (orders o JOIN audit_events a ON true) ON u.id=o.customer_id;","total","decimal").await;
    }
}
#[tokio::test]
async fn joined_tables_group_alias_exposes_its_output_shape() {
    for dialect in ["postgres", "mysql", "sqlite"] {
        typed(
            dialect,
            "SELECT g.to<cursor>tal FROM (users u JOIN orders o ON u.id=o.customer_id) g;",
            "total",
            "decimal",
        )
        .await;
        typed(
            dialect,
            "SELECT g.na<cursor>me FROM ((users u JOIN orders o ON u.id=o.customer_id)) g;",
            "name",
            "text",
        )
        .await;
    }
}
#[tokio::test]
async fn joined_tables_group_alias_hides_internal_and_future_qualifiers() {
    for sql in [
        "SELECT u.na<cursor>me FROM (users u JOIN orders o ON u.id=o.customer_id) g;",
        "SELECT g.id FROM (users u JOIN orders o ON g.to<cursor>tal=1) g;",
        "SELECT g.id FROM audit_events a, (users u JOIN orders o ON audit_<cursor>secret='x') g;",
    ] {
        let items = candidates("postgres", sql).await;
        assert!(
            !items
                .iter()
                .any(|i| i.kind == Some(CompletionItemKind::FIELD)),
            "{sql}: {items:?}"
        );
    }
}
#[tokio::test]
async fn joined_tables_natural_key_is_one_typed_candidate() {
    for dialect in ["postgres", "mysql", "sqlite"] {
        let sql = "SELECT i<cursor>d FROM users u NATURAL LEFT JOIN orders o;";
        let items = candidates(dialect, sql).await;
        let id: Vec<_> = items
            .iter()
            .filter(|i| {
                i.kind == Some(CompletionItemKind::FIELD) && i.filter_text.as_deref() == Some("id")
            })
            .collect();
        assert_eq!(id.len(), 1, "{dialect}: {items:?}");
        assert_eq!(id[0].label, "id");
        typed(dialect, sql, "id", "bigint").await;
    }
}
#[tokio::test]
async fn joined_tables_cte_projection_preserves_join_outputs() {
    typed("postgres","WITH q AS(SELECT g.* FROM (users u NATURAL JOIN orders o) g) SELECT q.to<cursor>tal FROM q;","total","decimal").await;
    typed(
        "postgres",
        "WITH q AS(SELECT * FROM users u JOIN orders o USING(id)) SELECT q.i<cursor>d FROM q;",
        "id",
        "bigint",
    )
    .await;
}
#[test]
fn joined_tables_nested_using_does_not_merge_unrelated_comma_input() {
    let sql = "SELECT id FROM users u, (orders o JOIN users v USING(id));";
    let ds = sql_inspection_diagnostics(sql, Some(&corpus_schema()), "postgres");
    assert_eq!(
        ds.iter()
            .filter(|d| d.code == Some(NumberOrString::String("OXIDE004".into())))
            .count(),
        1,
        "{ds:?}"
    );
}

#[tokio::test]
async fn joined_tables_sqlite_group_alias_keeps_inner_qualifiers_and_future_inner_inputs() {
    typed(
        "sqlite",
        "SELECT u.na<cursor>me FROM (users u JOIN orders o ON true) g;",
        "name",
        "text",
    )
    .await;
    typed("sqlite","SELECT * FROM (users u JOIN orders o ON a.audit_<cursor>secret='x') JOIN audit_events a ON true;","audit_secret","text").await;
    for sql in ["SELECT * FROM (users u LEFT JOIN orders o ON a.audit_<cursor>secret='x') JOIN audit_events a ON true;","SELECT * FROM (users u JOIN orders o ON a.audit_<cursor>secret='x') g JOIN audit_events a ON true;","SELECT * FROM audit_events a,(users u JOIN orders o ON a.audit_<cursor>secret='x');"] {
        let items=candidates("sqlite",sql).await;assert!(!items.iter().any(|i|i.kind==Some(CompletionItemKind::FIELD)),"{sql}: {items:?}");
    }
}
#[test]
fn joined_tables_diagnostics_use_group_output_and_own_on_namespace() {
    for (sql, count) in [
        ("SELECT id FROM users u NATURAL JOIN orders o", 0),
        ("SELECT id FROM (users u JOIN orders o USING(id)) g", 0),
        ("SELECT id FROM (users u JOIN orders o ON true) g", 1),
        (
            "SELECT id FROM (users u JOIN orders o USING(id)) g JOIN users v USING(id)",
            0,
        ),
        (
            "SELECT 1 FROM audit_events a,(users u JOIN orders o ON id=1) g",
            1,
        ),
    ] {
        let ds = sql_inspection_diagnostics(sql, Some(&corpus_schema()), "postgres");
        assert_eq!(
            ds.iter()
                .filter(|d| d.code == Some(NumberOrString::String("OXIDE004".into())))
                .count(),
            count,
            "{sql}: {ds:?}"
        );
    }
}
#[tokio::test]
async fn joined_tables_natural_profile_validation_is_not_generic_acceptance() {
    let registry = crate::dialects::DialectRegistry::new();
    for profile in [
        "postgres",
        "mysql",
        "sqlite",
        "sqlserver",
        "hive",
        "clickhouse",
    ] {
        let dialect = registry.get_by_name(profile).unwrap();
        let diagnostics = dialect
            .parse("SELECT * FROM users NATURAL JOIN orders", None)
            .await;
        assert_eq!(
            diagnostics.iter().any(
                |d| d.code == Some(NumberOrString::String("UNSUPPORTED_DIALECT_SYNTAX".into()))
            ),
            matches!(profile, "sqlserver" | "hive" | "clickhouse"),
            "{profile}: {diagnostics:?}"
        );
        let diagnostics = dialect
            .parse("SELECT * FROM users JOIN orders USING(id)", None)
            .await;
        assert_eq!(
            diagnostics.iter().any(
                |d| d.code == Some(NumberOrString::String("UNSUPPORTED_DIALECT_SYNTAX".into()))
            ),
            matches!(profile, "sqlserver" | "hive"),
            "{profile}: {diagnostics:?}"
        );
    }
}

#[tokio::test]
async fn joined_tables_group_outputs_feed_case_window_scalar_and_cte_inference() {
    for sql in [
        "WITH q AS(SELECT g.total,CASE WHEN g.total>0 THEN g.name ELSE 'none' END AS caption FROM (users u JOIN orders o USING(id)) g) SELECT q.cap<cursor>tion FROM q",
        "WITH q AS(SELECT (SELECT g.total) AS amount FROM (users u JOIN orders o USING(id)) g) SELECT q.amo<cursor>unt FROM q",
    ] {typed("postgres",sql,if sql.contains("caption"){"caption"}else{"amount"},if sql.contains("caption"){"text"}else{"decimal"}).await;}
    typed("postgres","WITH q AS(SELECT g.*,row_number() OVER(PARTITION BY g.id ORDER BY g.total) AS rn FROM (users u JOIN orders o USING(id)) g) SELECT q.to<cursor>tal FROM q","total","decimal").await;
}
#[tokio::test]
async fn joined_tables_quoted_alias_columns_and_ambiguous_group_fields() {
    typed(
        "postgres",
        "SELECT \"组\".to<cursor>tal FROM (users u JOIN orders o USING(id)) \"组\"",
        "total",
        "decimal",
    )
    .await;
    typed(
        "postgres",
        "SELECT g.ke<cursor>y FROM (users u JOIN orders o USING(id)) g(key)",
        "key",
        "bigint",
    )
    .await;
    let items = candidates(
        "postgres",
        "SELECT g.i<cursor>d FROM (users u JOIN orders o ON true) g",
    )
    .await;
    assert!(
        !items
            .iter()
            .any(|i| i.kind == Some(CompletionItemKind::FIELD)
                && i.filter_text.as_deref() == Some("id")),
        "{items:?}"
    );
}
#[tokio::test]
async fn joined_tables_schema_refresh_recomputes_natural_keys_and_group_members() {
    let h = configured(
        "postgres",
        "SELECT g.<cursor> FROM (users u NATURAL JOIN orders o) g",
    );
    assert!(complete_harness(&h)
        .await
        .iter()
        .any(|i| i.label == "total"));
    let server = h.service.inner();
    let id = *server.configured_file_schemas.get(h.uri.as_str()).unwrap();
    let mut schema = server.schema_manager.get(id).unwrap();
    schema
        .tables
        .iter_mut()
        .find(|t| t.name == "orders")
        .unwrap()
        .columns
        .iter_mut()
        .find(|c| c.name == "total")
        .unwrap()
        .name = "refreshed_total".into();
    assert!(server.schema_manager.update(id, schema));
    let items = complete_harness(&h).await;
    assert!(items.iter().any(|i| i.label == "refreshed_total"));
    assert!(!items.iter().any(|i| i.label == "total"));
}
#[tokio::test]
async fn joined_tables_deep_layout_budget_never_falls_back_to_physical_catalog() {
    let marked = format!(
        "SELECT g.<cursor> FROM {}users{} g",
        "(".repeat(40),
        ")".repeat(40)
    );
    let items = candidates("postgres", &marked).await;
    assert!(
        !items
            .iter()
            .any(|i| i.kind == Some(CompletionItemKind::FIELD)),
        "{items:?}"
    );
}

#[tokio::test]
async fn joined_tables_set_branches_bind_only_their_own_from_layout() {
    for sql in [
        "SELECT g.id FROM (users u JOIN orders o USING(id)) g UNION ALL SELECT q.to<cursor>tal FROM (users u JOIN orders o USING(id)) q",
        "SELECT q.to<cursor>tal FROM (users u JOIN orders o USING(id)) q UNION ALL SELECT g.id FROM (users u JOIN orders o USING(id)) g",
    ] {typed("postgres",sql,"total","decimal").await;}
    typed("postgres","SELECT g.id FROM (users u JOIN orders o USING(id)) g UNION ALL SELECT u.na<cursor>me FROM users u","name","text").await;
}
