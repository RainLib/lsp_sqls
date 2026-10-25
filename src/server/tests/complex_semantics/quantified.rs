use super::quoted_names::typed;

async fn output(dialect: &str, expression: &str, ty: &str) {
    typed(dialect, &format!("WITH result AS (SELECT {expression} AS value FROM users u) SELECT r.va<cursor>lue FROM result r"), "value", ty).await;
}

#[tokio::test]
async fn quantified_subqueries_keep_projected_dialect_result() {
    for (dialect, ty) in [("postgres", "boolean"), ("mysql", "bigint")] {
        for expression in [
            "u.id = ANY(SELECT o.id FROM orders o WHERE o.customer_id=u.id)",
            "u.id > SOME(SELECT o.id FROM orders o)",
            "u.id <= ALL(SELECT o.id FROM orders o)",
        ] {
            output(dialect, expression, ty).await;
        }
    }
}

#[tokio::test]
async fn postgres_array_quantifiers_compose_with_search_and_case() {
    for expression in [
        "u.id = ANY(ARRAY[1,2,NULL])",
        "u.id <> ALL(ARRAY[1,2])",
        "u.name ILIKE ANY(ARRAY['ox%','db%'])",
        "u.name ~ ALL(ARRAY['^ox','de$'])",
        "to_tsvector('english', u.name) @@ ANY(ARRAY[to_tsquery('english','oxide'),to_tsquery('english','dbx')])",
        "(to_tsvector('english', u.name) @@ to_tsquery('english','oxide')) = ANY(ARRAY[TRUE,FALSE])",
        "CASE WHEN u.id>0 THEN u.id = ANY(ARRAY[1,2]) ELSE FALSE END",
    ] {
        output("postgres", expression, "boolean").await;
    }
}

#[tokio::test]
async fn row_comparisons_keep_types_through_outer_projection() {
    for (dialect, ty) in [
        ("postgres", "boolean"),
        ("mysql", "bigint"),
        ("sqlite", "integer"),
    ] {
        for expression in [
            "(u.id,u.name) = (1,'oxide')",
            "(u.id,u.name) < (2,'dbx')",
            "(u.id,u.name) IN (SELECT id,status FROM orders)",
            "(u.id,u.name) = (SELECT o.id,o.status FROM orders o WHERE o.customer_id=u.id)",
        ] {
            output(dialect, expression, ty).await;
        }
    }
    output("postgres", "ROW(u.id,u.name) >= ROW(1,'oxide')", "boolean").await;
    output(
        "postgres",
        "(u.id,u.name) = ANY(SELECT id,status FROM orders)",
        "boolean",
    )
    .await;
}

#[tokio::test]
async fn quantified_conditions_keep_tsql_case_result_without_projectable_bool() {
    output(
        "sqlserver",
        "CASE WHEN u.id > ALL(SELECT id FROM orders) THEN 1 ELSE 0 END",
        "int",
    )
    .await;
    output("sqlserver", "u.id = ANY(SELECT id FROM orders)", "unknown").await;
}

#[tokio::test]
async fn cursors_inside_quantified_search_keep_inner_and_correlated_scope_types() {
    for (sql, name, ty) in [
        ("SELECT u.id FROM users u WHERE u.id=ANY(SELECT o.id FROM orders o WHERE o.sta<cursor>tus=u.name)", "status", "text"),
        ("SELECT u.id FROM users u WHERE u.id=ANY(SELECT o.id FROM orders o WHERE o.status=u.em<cursor>ail)", "email", "text"),
        ("SELECT u.name ILIKE ANY(ARRAY[COALESCE(u.na<cursor>me,'')]) FROM users u", "name", "text"),
        ("SELECT ARRAY[na<cursor>me] FROM users", "name", "text"),
        ("SELECT ARRAY [COALESCE(na<cursor>me,'')] FROM users", "name", "text"),
    ] { typed("postgres", sql, name, ty).await; }
}
