use super::*;

#[test]
fn variable_scopes_lazy_inspection_ranges_keep_utf16_and_suppression() {
    let prefix = "/*😀*/ DECLARE @金额 int;\r\n";
    let statement = "/*🧭*/ UPDATE users SET name=N'值';";
    let sql = format!("{prefix}{statement}\r\n-- noinspection OXIDE001\nDELETE FROM users;");
    let diagnostics = sql_inspection_diagnostics(&sql, None, "sqlserver");
    let mutations = diagnostics
        .iter()
        .filter(|d| {
            d.code
                == Some(tower_lsp::lsp_types::NumberOrString::String(
                    "OXIDE001".into(),
                ))
        })
        .collect::<Vec<_>>();
    assert_eq!(mutations.len(), 1, "{diagnostics:?}");
    assert_eq!(mutations[0].range.start, lsp_position_at_end(prefix));
    assert_eq!(
        mutations[0].range.end,
        lsp_position_at_end(&format!("{prefix}{statement}"))
    );
}

#[test]
fn semantic_inspections_cover_join_ambiguity_dialect_risk_and_suppression() {
    let shared_id = Column {
        name: "id".to_string(),
        data_type: "bigint".to_string(),
        ..Default::default()
    };
    let schema = Schema {
        id: SchemaId::new(),
        server: None,
        namespace: None,
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
    assert!(
        codes.contains(&"OXIDE004".to_string()),
        "{diagnostics:?}; tree={:?}",
        crate::parser::SqlParser::new()
            .parse("SELECT id FROM orders JOIN customers WHERE name ILIKE 'a%';")
            .tree
            .map(|t| t.root_node().to_sexp())
    );
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
