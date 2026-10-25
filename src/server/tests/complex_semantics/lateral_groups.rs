use super::*;
use tower_lsp::lsp_types::HoverParams;
async fn typed(dialect: &str, marked: &str, name: &str, ty: &str) {
    let h = configured(dialect, marked);
    let items = complete_harness(&h).await;
    assert!(
        items
            .iter()
            .any(|i| i.label == name && i.detail.as_deref().is_some_and(|d| d.contains(ty))),
        "{dialect}: {marked}: {items:?}"
    );
    let p: HoverParams = serde_json::from_value(
        serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
    )
    .unwrap();
    let hover = h.service.inner().hover(p).await.unwrap();
    assert!(
        serde_json::to_string(&hover).unwrap().contains(ty),
        "{dialect}: {marked}: {hover:?}"
    );
}
#[tokio::test]
async fn lateral_groups_closed_left_group_exposes_members_in_lateral_body() {
    typed("postgres","SELECT * FROM (users u JOIN orders o USING(id)) g CROSS JOIN LATERAL(SELECT g.to<cursor>tal) d","total","decimal").await;
    typed("postgres","SELECT * FROM (users u JOIN orders o USING(id)) g CROSS JOIN LATERAL(SELECT g.i<cursor>d) d","id","bigint").await;
}
#[tokio::test]
async fn lateral_groups_unaliased_left_inputs_work_with_lateral_and_apply() {
    for (dialect, join) in [
        ("postgres", "CROSS JOIN LATERAL"),
        ("mysql", "CROSS JOIN LATERAL"),
        ("sqlserver", "CROSS APPLY"),
        ("sqlserver", "OUTER APPLY"),
    ] {
        typed(dialect,&format!("SELECT * FROM (users u JOIN orders o ON u.id=o.id) {join}(SELECT o.to<cursor>tal) d"),"total","decimal").await;
    }
}
#[tokio::test]
async fn lateral_groups_projection_and_cte_keep_outer_group_types() {
    typed("postgres","WITH q AS(SELECT d.* FROM (users u JOIN orders o USING(id)) g CROSS JOIN LATERAL(SELECT g.total AS amount) d) SELECT q.am<cursor>ount FROM q","amount","decimal").await;
    typed("sqlserver","WITH q AS(SELECT d.* FROM (users u JOIN orders o ON u.id=o.id) OUTER APPLY(SELECT o.total AS amount) d) SELECT q.am<cursor>ount FROM q","amount","decimal").await;
}
#[tokio::test]
async fn lateral_groups_inner_lateral_can_see_eligible_enclosing_left_inputs() {
    typed("postgres","SELECT * FROM orders o JOIN (users u CROSS JOIN LATERAL(SELECT o.to<cursor>tal) d) ON true","total","decimal").await;
    typed("postgres","SELECT * FROM audit_events a,(users u CROSS JOIN LATERAL(SELECT a.audit_<cursor>secret) d)","audit_secret","text").await;
}
#[tokio::test]
async fn lateral_groups_do_not_see_own_group_alias_hidden_alias_or_future_input() {
    for marked in ["SELECT * FROM (users u CROSS JOIN LATERAL(SELECT g.to<cursor>tal) d JOIN orders o ON true) g", "SELECT * FROM (users u JOIN orders o USING(id)) g CROSS JOIN LATERAL(SELECT o.to<cursor>tal) d", "SELECT * FROM (users u CROSS JOIN LATERAL(SELECT o.to<cursor>tal) d) JOIN orders o ON true"] {
        let items=candidates("postgres",marked).await;assert!(!items.iter().any(|i|i.kind==Some(CompletionItemKind::FIELD)),"{marked}: {items:?}");
    }
}
#[tokio::test]
async fn lateral_groups_lateral_on_excludes_later_inputs() {
    for marked in ["SELECT * FROM orders o LEFT JOIN LATERAL(SELECT 1 AS value) d ON audit_<cursor>secret='x' JOIN audit_events a ON true", "SELECT * FROM audit_events a,orders o LEFT JOIN LATERAL(SELECT 1 AS value) d ON audit_<cursor>secret='x'"] {
        let items=candidates("postgres",marked).await;assert!(!items.iter().any(|i|i.kind==Some(CompletionItemKind::FIELD)),"{marked}: {items:?}");
    }
}
#[tokio::test]
async fn lateral_groups_anonymous_lateral_body_keeps_earlier_sources() {
    typed("postgres","SELECT * FROM (users u JOIN orders o USING(id)) g CROSS JOIN LATERAL(SELECT g.to<cursor>tal)","total","decimal").await;
}
#[tokio::test]
async fn lateral_groups_plain_derived_body_remains_isolated_from_sibling_group() {
    let marked =
        "SELECT * FROM (users u JOIN orders o USING(id)) g CROSS JOIN(SELECT g.to<cursor>tal) d";
    let items = candidates("postgres", marked).await;
    assert!(
        !items
            .iter()
            .any(|i| i.kind == Some(CompletionItemKind::FIELD)),
        "{items:?}"
    );
}

#[tokio::test]
async fn lateral_groups_merged_outer_key_is_one_common_typed_candidate() {
    let marked =
        "SELECT * FROM (users u JOIN orders o USING(id)) CROSS JOIN LATERAL(SELECT i<cursor>d) d";
    let items = candidates("postgres", marked).await;
    let keys: Vec<_> = items
        .iter()
        .filter(|i| {
            i.kind == Some(CompletionItemKind::FIELD) && i.filter_text.as_deref() == Some("id")
        })
        .collect();
    assert_eq!(keys.len(), 1, "{items:?}");
    assert_eq!(keys[0].label, "id");
    assert!(keys[0].detail.as_deref().unwrap().contains("bigint"));
}
#[tokio::test]
async fn lateral_groups_outer_apply_null_extends_literal_output() {
    for (join, nullable) in [
        ("CROSS APPLY", "Nullable: no"),
        ("OUTER APPLY", "Nullable: yes"),
    ] {
        let marked = format!("SELECT d.va<cursor>lue FROM (orders o {join}(SELECT 1 AS value) d)");
        let h = configured("sqlserver", &marked);
        let p: HoverParams = serde_json::from_value(
            serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
        )
        .unwrap();
        let hover = h.service.inner().hover(p).await.unwrap();
        assert!(
            serde_json::to_string(&hover).unwrap().contains(nullable),
            "{join}: {hover:?}"
        );
    }
}
#[tokio::test]
async fn lateral_groups_quote_and_column_aliases_keep_outer_member_types() {
    typed("postgres","SELECT * FROM (users u JOIN orders o USING(id)) \"组😀\"(key,label) CROSS JOIN LATERAL(SELECT \"组😀\".la<cursor>bel) d","label","text").await;
    typed("postgres","SELECT * FROM (users u JOIN orders o USING(id)) g CROSS JOIN LATERAL(SELECT 1 WHERE EXISTS(SELECT g.to<cursor>tal)) d","total","decimal").await;
}
#[tokio::test]
async fn lateral_groups_earlier_derived_output_reaches_later_lateral_after_group() {
    typed("postgres","SELECT b.am<cursor>ount FROM (users u JOIN orders o USING(id)) g CROSS JOIN LATERAL(SELECT g.total AS first_value) a CROSS JOIN LATERAL(SELECT a.first_value AS amount) b","amount","decimal").await;
}
#[tokio::test]
async fn lateral_groups_metadata_refresh_rebinds_group_without_catalog_mutation() {
    let h=configured("postgres","SELECT * FROM (users u JOIN orders o USING(id)) g CROSS JOIN LATERAL(SELECT g.to<cursor>tal) d");
    for ty in ["decimal", "numeric", "decimal"] {
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
            .data_type = ty.into();
        assert!(server.schema_manager.update(id, schema));
        let items = complete_harness(&h).await;
        assert!(
            items
                .iter()
                .any(|i| i.label == "total" && i.detail.as_deref().is_some_and(|d| d.contains(ty))),
            "{items:?}"
        );
        assert_eq!(server.schema_manager.get(id).unwrap().tables.len(), 3);
    }
}
