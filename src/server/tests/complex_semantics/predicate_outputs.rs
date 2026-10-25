use super::quoted_names::typed;
async fn output(dialect: &str, expression: &str, ty: &str) {
    typed(dialect,&format!("WITH result AS (SELECT {expression} AS value FROM users u) SELECT r.va<cursor>lue FROM result r"),"value",ty).await;
}
#[tokio::test]
async fn exists_and_not_exists_keep_projected_predicate_type() {
    for (dialect, ty) in [
        ("postgres", "boolean"),
        ("mysql", "bigint"),
        ("sqlite", "integer"),
    ] {
        output(
            dialect,
            "EXISTS(SELECT o.id FROM orders o WHERE o.customer_id=u.id)",
            ty,
        )
        .await;
        output(
            dialect,
            "NOT EXISTS(SELECT o.id FROM orders o WHERE o.customer_id=u.id)",
            ty,
        )
        .await;
    }
}
#[tokio::test]
async fn membership_lists_and_correlated_subqueries_keep_predicate_type() {
    for (dialect, ty) in [
        ("postgres", "boolean"),
        ("mysql", "bigint"),
        ("sqlite", "integer"),
    ] {
        output(dialect, "u.id IN (1,2,NULL)", ty).await;
        output(
            dialect,
            "u.id NOT IN (SELECT o.customer_id FROM orders o)",
            ty,
        )
        .await;
    }
}
#[tokio::test]
async fn comparisons_null_tests_and_distinct_tests_keep_type() {
    for expression in [
        "u.name = 'oxide'",
        "u.id IS NULL",
        "u.id IS NOT NULL",
        "u.id IS DISTINCT FROM NULL",
    ] {
        output("postgres", expression, "boolean").await;
    }
    output("mysql", "u.name <=> NULL", "bigint").await;
    output("mysql", "u.id > 0", "bigint").await;
    output("sqlite", "u.id IS NULL", "integer").await;
}
#[tokio::test]
async fn logical_search_combinations_keep_outer_predicate_type() {
    output("postgres","u.id>0 AND (u.name IS NOT NULL OR EXISTS(SELECT * FROM orders o WHERE o.customer_id=u.id))","boolean").await;
    output(
        "mysql",
        "u.id>0 AND NOT EXISTS(SELECT * FROM orders o WHERE o.customer_id=u.id)",
        "bigint",
    )
    .await;
}
#[tokio::test]
async fn case_over_search_predicates_keeps_integer_result_types() {
    output(
        "sqlserver",
        "CASE WHEN EXISTS(SELECT * FROM orders o WHERE o.customer_id=u.id) THEN 1 ELSE 0 END",
        "int",
    )
    .await;
    output(
        "mysql",
        "CASE WHEN u.id IN (SELECT customer_id FROM orders) THEN 1 ELSE 0 END",
        "bigint",
    )
    .await;
}
#[tokio::test]
async fn unsupported_postgres_operand_overloads_do_not_invent_comparison_type() {
    output("postgres", "mystery() = mystery()", "unknown").await;
    output("postgres", "u.payload = u.payload", "unknown").await;
    output("sqlserver", "u.id > 0", "unknown").await;
}
