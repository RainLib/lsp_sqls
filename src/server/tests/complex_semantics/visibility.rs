use super::*;
use tower_lsp::lsp_types::Hover;

async fn hover_at(dialect: &str, sql: &str) -> Option<Hover> {
    let harness = configured(dialect, sql);
    harness
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
}

#[tokio::test]
async fn unqualified_hover_binds_to_active_row_source() {
    for dialect in DIALECTS.iter().copied() {
        let hover = hover_at(dialect, "SELECT i<cursor>d FROM orders")
            .await
            .expect("active orders.id");
        let value = serde_json::to_string(&hover).unwrap();
        assert!(value.contains("orders.id"), "{dialect}: {value}");
        assert!(!value.contains("users.id"), "{dialect}: {value}");
    }
}

#[tokio::test]
async fn hover_does_not_resolve_invisible_or_ambiguous_columns() {
    for dialect in DIALECTS.iter().copied() {
        for sql in [
            "SELECT audit_<cursor>secret FROM users u",
            "SELECT i<cursor>d FROM users u JOIN orders o ON u.id=o.customer_id",
            "SELECT i<cursor>d FROM users u JOIN users v ON u.id=v.id",
            "SELECT na<cursor>me",
            "SELECT u.id FROM users u WHERE EXISTS(SELECT to<cursor>tal FROM users v)",
        ] {
            let hover = hover_at(dialect, sql).await;
            assert!(hover.is_none(), "{dialect}: {sql}: {hover:?}");
        }
    }
}

#[tokio::test]
async fn column_named_like_catalog_table_is_still_a_column() {
    for dialect in DIALECTS.iter().copied() {
        let sql = "SELECT us<cursor>ers FROM (SELECT id AS users FROM app.users) d";
        let hover = hover_at(dialect, sql)
            .await
            .unwrap_or_else(|| panic!("{dialect}: {sql}"));
        let value = serde_json::to_string(&hover).unwrap();
        assert!(
            value.contains("Column") && value.contains("bigint"),
            "{dialect}: {value}"
        );
        assert!(!value.contains("Table"), "{dialect}: {value}");
    }
}

#[tokio::test]
async fn inner_alias_shadow_and_correlated_outer_reference_remain_distinct() {
    for dialect in DIALECTS.iter().copied() {
        for (sql, expected, forbidden) in [
            ("SELECT * FROM users u WHERE EXISTS(SELECT u.to<cursor>tal FROM orders u WHERE u.id>0)","total","name"),
            ("SELECT * FROM users u WHERE EXISTS(SELECT u.na<cursor>me FROM orders o WHERE o.customer_id=u.id)","name","total"),
        ] {
            let items=candidates(dialect,sql).await;
            assert!(items.iter().any(|item| item.label==expected),"{dialect}: {sql}: {items:?}");
            assert!(!items.iter().any(|item| item.label==forbidden),"{dialect}: {sql}: {items:?}");
            let hover=hover_at(dialect,sql).await.expect("scoped alias hover");
            assert!(serde_json::to_string(&hover).unwrap().contains(expected));
        }
    }
}

#[tokio::test]
async fn nested_with_output_keeps_inner_names_and_types() {
    for dialect in ["postgres", "mysql", "sqlite"] {
        let sql="WITH c AS (SELECT id AS outer_key FROM app.users) SELECT x.inner_<cursor>name FROM (WITH c AS (SELECT name AS inner_name FROM app.users) SELECT c.* FROM c) x";
        let items = candidates(dialect, sql).await;
        let item = items
            .iter()
            .find(|item| item.label == "inner_name")
            .unwrap_or_else(|| panic!("{dialect}: {items:?}"));
        assert!(
            item.detail
                .as_ref()
                .is_some_and(|detail| detail.contains("text")),
            "{dialect}: {item:?}"
        );
        assert!(!items.iter().any(|item| item.label == "outer_key"));
    }
}

#[tokio::test]
async fn nested_with_shadowing_does_not_escape_to_outer_query() {
    for dialect in ["postgres", "mysql", "sqlite"] {
        for sql in [
            "WITH c AS (SELECT id AS outer_key FROM app.users) SELECT c.outer_<cursor>key FROM c WHERE EXISTS (WITH c AS (SELECT name AS inner_name FROM app.users) SELECT 1 FROM c)",
            "WITH c AS (SELECT id AS outer_key FROM app.users) SELECT * FROM c WHERE EXISTS (WITH c AS (SELECT name AS inner_name FROM app.users) SELECT c.inner_<cursor>name FROM c)",
        ] {
            let expected=if sql.contains("outer_<cursor>"){"outer_key"}else{"inner_name"};
            let items=candidates(dialect,sql).await;
            assert!(items.iter().any(|item|item.label==expected),"{dialect}: {sql}: {items:?}");
        }
    }
}
