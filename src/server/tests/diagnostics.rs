use super::*;

#[test]
fn semantic_inspections_cover_join_ambiguity_dialect_risk_and_suppression() {
    let shared_id = Column {
        name: "id".to_string(),
        data_type: "bigint".to_string(),
        ..Default::default()
    };
    let schema = Schema {
        id: SchemaId::new(),
        catalog: None,
        database: "app".to_string(),
        server_version: None,
        tables: vec![
            Table {
                name: "orders".to_string(),
                columns: vec![shared_id.clone()],
                ..Default::default()
            },
            Table {
                name: "customers".to_string(),
                columns: vec![shared_id],
                ..Default::default()
            },
        ],
        functions: Vec::new(),
        source_uri: None,
    };
    let diagnostics = sql_inspection_diagnostics(
        "SELECT id FROM orders JOIN customers WHERE name ILIKE 'a%';",
        Some(&schema),
        "mysql",
    );
    let codes = diagnostic_codes(&diagnostics);
    assert!(codes.contains(&"OXIDE002".to_string()), "{diagnostics:?}");
    assert!(codes.contains(&"OXIDE004".to_string()), "{diagnostics:?}");
    assert!(codes.contains(&"OXIDE005".to_string()), "{diagnostics:?}");

    let suppressed = sql_inspection_diagnostics(
        "-- noinspection OXIDE001\nUPDATE orders SET id = 1;",
        Some(&schema),
        "postgres",
    );
    assert!(
        !diagnostic_codes(&suppressed).contains(&"OXIDE001".to_string()),
        "{suppressed:?}"
    );
}
