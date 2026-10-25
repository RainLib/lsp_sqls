use super::quoted_names::typed;
#[tokio::test]
async fn correlated_scalar_output_keeps_type_in_outer_candidates() {
    for dialect in ["postgres", "mysql", "sqlite", "sqlserver"] {
        typed(dialect,"WITH result AS (SELECT (SELECT u.name) AS value FROM users u) SELECT r.va<cursor>lue FROM result r","value","text").await;
    }
}
#[tokio::test]
async fn nested_scalar_and_expression_outputs_keep_types() {
    for sql in [
        "WITH result AS (SELECT (SELECT (SELECT u.name)) AS value FROM users u) SELECT r.va<cursor>lue FROM result r",
        "WITH result AS (SELECT COALESCE((SELECT u.name), 'fallback') AS value FROM users u) SELECT r.va<cursor>lue FROM result r",
        "WITH result AS (SELECT CASE WHEN u.id>0 THEN (SELECT u.name) ELSE 'fallback' END AS value FROM users u) SELECT r.va<cursor>lue FROM result r",
    ] { typed("postgres",sql,"value","text").await; }
    typed("postgres","WITH result AS (SELECT (SELECT u.id)+1 AS value FROM users u) SELECT r.va<cursor>lue FROM result r","value","bigint").await;
}
#[tokio::test]
async fn scalar_local_binding_overrides_outer_same_alias() {
    typed("postgres","WITH result AS (SELECT (SELECT u.total FROM orders u LIMIT 1) AS value FROM users u) SELECT r.va<cursor>lue FROM result r","value","decimal").await;
    typed("postgres","WITH result AS (SELECT (SELECT name FROM orders LIMIT 1) AS value FROM users u) SELECT r.va<cursor>lue FROM result r","value","text").await;
}
#[tokio::test]
async fn invalid_scalar_arity_and_unknown_bindings_remain_unknown() {
    for body in [
        "SELECT id,name FROM users",
        "SELECT u.name FROM missing_table u",
        "SELECT id FROM users u JOIN orders o ON u.id=o.customer_id",
    ] {
        let marked=format!("WITH result AS (SELECT ({body}) AS value FROM users u) SELECT r.va<cursor>lue FROM result r");
        typed("postgres", &marked, "value", "unknown").await;
    }
}

#[tokio::test]
async fn scalar_with_queries_and_aggregates_keep_output_types() {
    for dialect in ["postgres", "mysql"] {
        typed(dialect,"WITH result AS (SELECT (WITH inner_value AS (SELECT id FROM users) SELECT id FROM inner_value LIMIT 1) AS value) SELECT r.va<cursor>lue FROM result r","value","bigint").await;
        typed(dialect,"WITH result AS (SELECT (SELECT COUNT(*) FROM orders o WHERE o.customer_id=u.id) AS value FROM users u) SELECT r.va<cursor>lue FROM result r","value","bigint").await;
    }
}
#[tokio::test]
async fn scalar_inside_derived_query_respects_correlation_boundaries() {
    typed(
        "postgres",
        "SELECT d.va<cursor>lue FROM users u CROSS JOIN LATERAL(SELECT (SELECT u.name) AS value) d",
        "value",
        "text",
    )
    .await;
    typed(
        "postgres",
        "SELECT d.va<cursor>lue FROM users u CROSS JOIN(SELECT (SELECT u.name) AS value) d",
        "value",
        "unknown",
    )
    .await;
}
