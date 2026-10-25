use super::quoted_names::typed;
#[tokio::test]
async fn fromless_where_scalar_keeps_correlated_output_type() {
    for dialect in ["postgres", "mysql", "sqlserver"] {
        typed(dialect,"WITH result AS (SELECT (SELECT u.id WHERE 1=0 UNION ALL SELECT u.id) AS value FROM users u) SELECT r.va<cursor>lue FROM result r","value","bigint").await;
    }
}
#[tokio::test]
async fn nested_set_scalar_keeps_grouped_correlated_output_type() {
    typed("postgres","WITH result AS (SELECT ((SELECT u.id WHERE FALSE UNION ALL SELECT u.id WHERE FALSE) UNION ALL (SELECT u.id WHERE FALSE UNION ALL SELECT u.id)) AS value FROM users u) SELECT r.va<cursor>lue FROM result r","value","bigint").await;
    typed("postgres","WITH result AS (((SELECT id FROM users) UNION ALL (SELECT id FROM orders)) INTERSECT (SELECT id FROM users)) SELECT r.i<cursor>d FROM result r","id","bigint").await;
}
