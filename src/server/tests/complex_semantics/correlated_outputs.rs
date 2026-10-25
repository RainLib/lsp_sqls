use super::*;
async fn typed(dialect: &str, marked: &str, name: &str, ty: &str) {
    let harness = configured(dialect, marked);
    let items = complete_harness(&harness).await;
    let item = items
        .iter()
        .find(|item| item.label == name)
        .unwrap_or_else(|| panic!("{dialect}: {marked}: {items:?}"));
    assert!(
        item.detail
            .as_ref()
            .is_some_and(|detail| detail.contains(ty)),
        "{dialect}: {marked}: {item:?}"
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
        .expect("correlated output hover");
    assert!(
        serde_json::to_string(&hover).unwrap().contains(ty),
        "{dialect}: {marked}: {hover:?}"
    );
}
#[tokio::test]
async fn apply_and_lateral_outputs_keep_outer_column_types() {
    for (dialect, join) in [
        ("postgres", "CROSS JOIN LATERAL"),
        ("mysql", "CROSS JOIN LATERAL"),
        ("sqlserver", "CROSS APPLY"),
        ("sqlserver", "OUTER APPLY"),
    ] {
        let sql =
            format!("SELECT d.am<cursor>ount FROM orders o {join}(SELECT o.total AS amount) d");
        typed(dialect, &sql, "amount", "decimal").await;
    }
}
#[tokio::test]
async fn lateral_star_and_cte_outputs_keep_positional_types() {
    typed("postgres","WITH result AS (SELECT d.* FROM orders o CROSS JOIN LATERAL(SELECT o.*) d) SELECT r.to<cursor>tal FROM result r","total","decimal").await;
    typed("sqlserver","WITH result(amount,output_key) AS (SELECT d.* FROM orders o CROSS APPLY(SELECT o.total,o.id) d) SELECT r.am<cursor>ount FROM result r","amount","decimal").await;
}
#[tokio::test]
async fn subsequent_lateral_outputs_can_depend_on_previous_derived_outputs() {
    typed("postgres","SELECT b.am<cursor>ount FROM orders o CROSS JOIN LATERAL(SELECT o.total AS first_value) a CROSS JOIN LATERAL(SELECT a.first_value AS amount) b","amount","decimal").await;
    typed("sqlserver","SELECT b.output_<cursor>key FROM orders o CROSS APPLY(SELECT o.id AS first_key) a OUTER APPLY(SELECT a.first_key AS output_key) b","output_key","bigint").await;
}
#[tokio::test]
async fn unqualified_lateral_projection_uses_inner_column_before_outer_column() {
    typed(
        "postgres",
        "SELECT d.am<cursor>ount FROM orders o CROSS JOIN LATERAL(SELECT total AS amount) d",
        "amount",
        "decimal",
    )
    .await;
    typed("postgres","SELECT d.am<cursor>ount FROM orders o CROSS JOIN LATERAL(SELECT total AS amount FROM (SELECT name AS total FROM users) inner_row) d","amount","text").await;
}
#[tokio::test]
async fn plain_derived_and_right_lateral_outputs_do_not_infer_invisible_columns() {
    for marked in ["SELECT d.am<cursor>ount FROM orders o CROSS JOIN(SELECT o.total AS amount) d","SELECT d.am<cursor>ount FROM users u CROSS JOIN LATERAL(SELECT o.total AS amount) d CROSS JOIN orders o"] {
        let items=candidates("postgres",marked).await;
        let item=items.iter().find(|item|item.label=="amount").expect("explicit projection name remains available");
        assert!(item.detail.as_ref().is_some_and(|detail|detail.contains("unknown")),"{marked}: {item:?}");
    }
}
#[tokio::test]
async fn aliased_lateral_outputs_preserve_each_invocation_type() {
    typed("postgres","SELECT a.am<cursor>ount FROM orders o CROSS JOIN LATERAL(SELECT o.total AS amount) a CROSS JOIN LATERAL(SELECT o.id AS amount) b","amount","decimal").await;
    typed("postgres","SELECT b.am<cursor>ount FROM orders o CROSS JOIN LATERAL(SELECT o.total AS amount) a CROSS JOIN LATERAL(SELECT o.id AS amount) b","amount","bigint").await;
}

#[tokio::test]
async fn aggregate_and_window_outputs_flow_through_lateral_and_ctes() {
    typed("postgres","WITH amounts AS (SELECT o.id,d.amount FROM orders o CROSS JOIN LATERAL(SELECT COALESCE(SUM(i.total),0) AS amount FROM orders i WHERE i.customer_id=o.customer_id) d), ranked AS (SELECT id,amount,ROW_NUMBER() OVER(ORDER BY amount DESC) AS pos FROM amounts) SELECT r.am<cursor>ount FROM ranked r","amount","numeric").await;
}
#[tokio::test]
async fn correlated_output_refreshes_same_uri_without_mutating_catalog() {
    let first = "SELECT a.va<cursor>lue FROM users u CROSS JOIN LATERAL(SELECT u.id AS value) a";
    let harness = configured("postgres", first);
    let server = harness.service.inner();
    let schema_id = *server
        .configured_file_schemas
        .get(harness.uri.as_str())
        .unwrap();
    for (revision, projection, ty) in [
        (2, "id", "bigint"),
        (3, "name", "text"),
        (4, "id", "bigint"),
    ] {
        let sql = format!(
            "SELECT a.value FROM users u CROSS JOIN LATERAL(SELECT u.{projection} AS value) a"
        );
        server
            .document_manager
            .update(harness.uri.to_string(), sql, revision);
        let items = complete_harness(&harness).await;
        let item = items.iter().find(|item| item.label == "value").unwrap();
        assert!(
            item.detail
                .as_ref()
                .is_some_and(|detail| detail.contains(ty)),
            "{item:?}"
        );
        let stored = server.schema_manager.get(schema_id).unwrap();
        assert_eq!(stored.tables.len(), 3);
        assert!(stored
            .tables
            .iter()
            .all(|table| !crate::relation_bindings::query_local(table)));
    }
}
