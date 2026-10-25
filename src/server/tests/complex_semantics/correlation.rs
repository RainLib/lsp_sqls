use super::*;
use tower_lsp::lsp_types::Hover;
async fn hover_at(dialect: &str, sql: &str) -> Option<Hover> {
    let harness = configured(dialect, sql);
    harness
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
}
async fn typed(dialect: &str, sql: &str, column: &str, ty: &str) {
    let hover = hover_at(dialect, sql)
        .await
        .unwrap_or_else(|| panic!("{dialect}: {sql}"));
    assert!(
        serde_json::to_string(&hover).unwrap().contains(ty),
        "{dialect}: {sql}: {hover:?}"
    );
    let items = candidates(dialect, sql).await;
    assert!(
        items.iter().any(|item| (item.label == column
            || item.label.ends_with(&format!(".{column}")))
            && item
                .detail
                .as_ref()
                .is_some_and(|detail| detail.contains(ty))),
        "{dialect}: {sql}: {items:?}"
    );
}
#[tokio::test]
async fn unqualified_correlated_columns_search_from_inside_to_outside() {
    for dialect in DIALECTS.iter().copied() {
        typed(
            dialect,
            "SELECT * FROM orders o WHERE EXISTS(SELECT to<cursor>tal FROM users u)",
            "total",
            "decimal",
        )
        .await;
        typed(dialect,"SELECT * FROM orders o WHERE EXISTS(SELECT 1 FROM users u WHERE EXISTS(SELECT to<cursor>tal))","total","decimal").await;
    }
}
#[tokio::test]
async fn inner_column_and_alias_shadow_outer_binding() {
    for dialect in DIALECTS.iter().copied() {
        typed(dialect,"SELECT * FROM orders o WHERE EXISTS(SELECT to<cursor>tal FROM (SELECT name AS total FROM users) d)","total","text").await;
        let sql = "SELECT * FROM orders o WHERE EXISTS(SELECT o.to<cursor>tal FROM users o)";
        assert!(hover_at(dialect, sql).await.is_none(), "{dialect}: {sql}");
        assert!(!candidates(dialect, sql)
            .await
            .iter()
            .any(|item| item.label == "total"));
    }
}
#[tokio::test]
async fn qualified_columns_require_visible_unhidden_relations() {
    for dialect in DIALECTS.iter().copied() {
        for sql in [
            "SELECT users.na<cursor>me FROM orders o",
            "SELECT users.na<cursor>me FROM users u",
            "SELECT app.users.na<cursor>me FROM app.users u",
        ] {
            assert!(hover_at(dialect, sql).await.is_none(), "{dialect}: {sql}");
            assert!(
                !candidates(dialect, sql)
                    .await
                    .iter()
                    .any(|item| item.label == "name"),
                "{dialect}: {sql}"
            );
        }
        typed(
            dialect,
            "SELECT app.users.na<cursor>me FROM app.users",
            "name",
            "text",
        )
        .await;
    }
}
#[tokio::test]
async fn plain_derived_query_cannot_correlate_to_sibling_source() {
    for dialect in ["postgres", "mysql", "sqlite"] {
        for sql in [
            "SELECT * FROM orders o JOIN (SELECT o.to<cursor>tal) d ON TRUE",
            "SELECT * FROM orders o JOIN (SELECT to<cursor>tal) d ON TRUE",
        ] {
            assert!(hover_at(dialect, sql).await.is_none(), "{dialect}: {sql}");
            assert!(
                !candidates(dialect, sql)
                    .await
                    .iter()
                    .any(|item| item.label == "total" || item.label == "o.total"),
                "{dialect}: {sql}"
            );
        }
        typed(
            dialect,
            "SELECT * FROM orders o WHERE EXISTS(SELECT 1 FROM (SELECT o.to<cursor>tal) d)",
            "total",
            "decimal",
        )
        .await;
    }
}
#[tokio::test]
async fn lateral_query_sees_preceding_sources_but_not_following_sources() {
    for dialect in ["postgres", "mysql"] {
        typed(
            dialect,
            "SELECT * FROM orders o CROSS JOIN LATERAL(SELECT o.to<cursor>tal) d",
            "total",
            "decimal",
        )
        .await;
        typed(
            dialect,
            "SELECT * FROM orders o CROSS JOIN LATERAL(SELECT to<cursor>tal) d",
            "total",
            "decimal",
        )
        .await;
        let sql="SELECT * FROM users u CROSS JOIN LATERAL(SELECT o.to<cursor>tal) d CROSS JOIN orders o";
        assert!(hover_at(dialect, sql).await.is_none(), "{dialect}: {sql}");
        assert!(
            !candidates(dialect, sql)
                .await
                .iter()
                .any(|item| item.label == "total"),
            "{dialect}: {sql}"
        );
    }
}
#[tokio::test]
async fn cte_body_cannot_read_its_own_main_query_rows() {
    for dialect in ["postgres", "mysql", "sqlite"] {
        let sql = "WITH c AS (SELECT o.to<cursor>tal) SELECT * FROM orders o";
        assert!(hover_at(dialect, sql).await.is_none(), "{dialect}: {sql}");
        assert!(
            !candidates(dialect, sql)
                .await
                .iter()
                .any(|item| item.label == "total"),
            "{dialect}: {sql}"
        );
    }
}

#[tokio::test]
async fn inner_ambiguity_does_not_fall_back_to_a_unique_outer_column() {
    for dialect in DIALECTS.iter().copied() {
        let sql="SELECT * FROM orders o WHERE EXISTS(SELECT to<cursor>tal FROM (SELECT name AS total FROM users) a CROSS JOIN (SELECT name AS total FROM users) b)";
        assert!(hover_at(dialect, sql).await.is_none(), "{dialect}: {sql}");
        let items = candidates(dialect, sql).await;
        assert!(
            !items.iter().any(|item| item.label == "o.total"),
            "{dialect}: {items:?}"
        );
    }
}
#[tokio::test]
async fn correlated_candidate_keeps_quoted_outer_alias_insertion() {
    let sql = "SELECT * FROM orders AS \"外😀\" WHERE EXISTS(SELECT to<cursor>tal FROM users u)";
    typed("postgres", sql, "total", "decimal").await;
    let items = candidates("postgres", sql).await;
    let item = items
        .iter()
        .find(|item| item.label == "外😀.total")
        .expect("quoted outer alias candidate");
    assert_eq!(item.insert_text.as_deref(), Some("\"外😀\".total"));
}

#[tokio::test]
async fn apply_and_lateral_join_directions_constrain_left_correlation() {
    for sql in [
        "SELECT * FROM orders o CROSS APPLY(SELECT o.to<cursor>tal) d",
        "SELECT * FROM orders o OUTER APPLY(SELECT to<cursor>tal) d",
    ] {
        typed("sqlserver", sql, "total", "decimal").await;
    }
    for direction in ["RIGHT", "FULL", "RIGHT OUTER", "FULL OUTER"] {
        let sql = format!(
            "SELECT * FROM orders o {direction} JOIN LATERAL(SELECT o.to<cursor>tal) d ON TRUE"
        );
        assert!(hover_at("postgres", &sql).await.is_none(), "{sql}");
        assert!(
            !candidates("postgres", &sql)
                .await
                .iter()
                .any(|item| item.label == "total"),
            "{sql}"
        );
    }
}
