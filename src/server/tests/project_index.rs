use super::*;

#[test]
fn project_sql_index_tracks_ddl_calls_and_physical_relation_references() {
    let sql = r#"-- FROM ignored.orders
CREATE VIEW reporting.active_orders AS
SELECT * FROM app.orders;
SELECT 'FROM secret.orders';
WITH recent AS (SELECT * FROM app.orders)
SELECT * FROM recent;
CALL ops.refresh_orders();
CREATE FUNCTION ops.compute_total() RETURNS integer AS $$ SELECT 1 $$ LANGUAGE SQL;
SELECT ops.compute_total();
SELECT * FROM recent;"#;
    let occurrences = project_sql_symbol_occurrences(sql);

    assert!(occurrences.iter().any(|occurrence| {
        occurrence.role == ProjectSqlSymbolRole::Definition
            && occurrence.kind == ProjectSqlSymbolKind::View
            && occurrence.normalized_name == "reporting.active_orders"
    }));
    assert_eq!(
        occurrences
            .iter()
            .filter(|occurrence| {
                occurrence.role == ProjectSqlSymbolRole::Reference
                    && occurrence.normalized_name == "app.orders"
            })
            .count(),
        2
    );
    assert!(occurrences.iter().any(|occurrence| {
        occurrence.kind == ProjectSqlSymbolKind::Procedure
            && occurrence.normalized_name == "ops.refresh_orders"
    }));
    assert!(occurrences.iter().any(|occurrence| {
        occurrence.kind == ProjectSqlSymbolKind::Function
            && occurrence.role == ProjectSqlSymbolRole::Definition
            && occurrence.normalized_name == "ops.compute_total"
    }));
    assert!(occurrences.iter().any(|occurrence| {
        occurrence.kind == ProjectSqlSymbolKind::Function
            && occurrence.role == ProjectSqlSymbolRole::Reference
            && occurrence.normalized_name == "ops.compute_total"
    }));
    assert_eq!(
        occurrences
            .iter()
            .filter(|occurrence| occurrence.normalized_name == "recent")
            .count(),
        1,
        "a CTE name must become physical again outside its statement"
    );
    assert!(!occurrences.iter().any(|occurrence| {
        matches!(
            occurrence.normalized_name.as_str(),
            "ignored.orders" | "secret.orders"
        )
    }));
}

#[test]
fn project_sql_index_preserves_quoted_names_and_utf16_selection_ranges() {
    let sql = "SELECT 'emoji 馃榾', * FROM [AppDb]..[orders]";
    let occurrence = project_sql_symbol_occurrences(sql)
        .into_iter()
        .find(|occurrence| occurrence.normalized_name == "appdb.dbo.orders")
        .expect("SQL Server relation should be indexed");
    let orders_start = sql.find("orders").unwrap();

    assert_eq!(occurrence.name, "orders");
    assert_eq!(
        occurrence.range.start.character,
        sql[..orders_start].encode_utf16().count() as u32
    );
    assert_eq!(
        occurrence.range.end.character,
        sql[..orders_start + "orders".len()].encode_utf16().count() as u32
    );
}

#[test]
fn project_sql_symbol_matching_uses_qualified_suffixes_and_object_families() {
    let relation = |normalized_name: &str, kind| ProjectSqlSymbolOccurrence {
        name: normalized_name.split('.').next_back().unwrap().to_string(),
        normalized_name: normalized_name.to_string(),
        kind,
        role: ProjectSqlSymbolRole::Reference,
        range: Range::default(),
    };
    let qualified = relation("catalog.app.orders", ProjectSqlSymbolKind::Table);
    let schema_qualified = relation("app.orders", ProjectSqlSymbolKind::View);
    let unqualified = relation("orders", ProjectSqlSymbolKind::Table);
    let routine = relation("orders", ProjectSqlSymbolKind::Procedure);

    assert!(project_sql_symbols_match(&qualified, &schema_qualified));
    assert!(project_sql_symbols_match(&qualified, &unqualified));
    assert!(!project_sql_symbols_match(&qualified, &routine));
}

#[test]
fn project_sql_index_is_bounded_for_oversized_documents() {
    let sql = format!(
        "SELECT * FROM orders;{}",
        " ".repeat(PROJECT_SQL_INDEX_MAX_BYTES)
    );
    assert!(project_sql_symbol_occurrences(&sql).is_empty());
}
