use super::*;
use tower_lsp::lsp_types::{InlayHintParams, NumberOrString, TextDocumentIdentifier};

fn setup(profile: &str, sql: &str) -> Harness {
    let h = configured(profile, &format!("{sql}<cursor>"));
    let manager = &h.service.inner().schema_manager;
    let id = *h
        .service
        .inner()
        .configured_file_schemas
        .get(h.uri.as_str())
        .unwrap();
    let mut base = manager.get(id).unwrap();
    base.catalog = Some("db".into());
    base.database = "app".into();
    base.tables = vec![table("ledger", "local_value"), table("collision", "shared")];
    manager.update(id, base.clone());
    for (server, catalog, schema, column) in [
        (None, "other", "app", "other_value"),
        (None, "db", "sales", "sales_value"),
        (None, "db", "dbo", "dbo_value"),
        (Some("remote"), "db", "app", "remote_value"),
    ] {
        let mut s = base.clone();
        s.id = SchemaId::new();
        s.server = server.map(str::to_string);
        s.catalog = Some(catalog.into());
        s.database = schema.into();
        s.tables = vec![table("ledger", column), table("collision", "shared")];
        manager.register(s);
    }
    h
}
fn table(name: &str, column: &str) -> Table {
    Table {
        name: name.into(),
        columns: vec![Column {
            name: column.into(),
            data_type: "int".into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}
fn context(h: &Harness, default: Option<&str>, complete: bool, primary: bool) {
    let manager = &h.service.inner().schema_manager;
    for id in manager.list_ids() {
        let mut s = manager.get(id).unwrap();
        if s.server.is_none() && s.catalog.as_deref() == Some("db") {
            s.namespace = default.map(|n| {
                serde_json::from_value(
                    serde_json::json!({"default_schema":n,"tables_complete":complete}),
                )
                .unwrap()
            });
            if s.database == "sales" && !primary {
                s.tables.retain(|t| t.name != "ledger");
            }
            manager.update(id, s);
        }
    }
}
async fn hints(h: &Harness) -> Vec<String> {
    let result = h
        .service
        .inner()
        .inlay_hint(InlayHintParams {
            text_document: TextDocumentIdentifier { uri: h.uri.clone() },
            range: Range::new(Position::new(0, 0), Position::new(u32::MAX, 0)),
            work_done_progress_params: Default::default(),
        })
        .await
        .unwrap()
        .unwrap();
    result
        .into_iter()
        .map(|i| {
            serde_json::to_value(i.label)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}
async fn ambiguity(h: &Harness) -> usize {
    let service = h.service.inner();
    let text = service.document_manager.get(h.uri.as_str()).unwrap();
    let dialect = service.get_dialect_for_file(h.uri.as_str()).unwrap();
    let schema = service.get_schema_for_file(h.uri.as_str());
    crate::server::diagnostics::document_diagnostics_with_metadata(
        &*dialect,
        &text,
        schema.as_ref(),
        Some(&service.schema_manager),
    )
    .await
    .iter()
    .filter(|d| d.code == Some(NumberOrString::String("OXIDE004".into())))
    .count()
}

#[tokio::test]
async fn multi_statement_insert_hints_keep_catalog_schema_and_server_sources() {
    let sql="/* 😀 */\r\nINSERT INTO db.app.ledger VALUES(1);\r\nINSERT INTO other.app.ledger VALUES(2); INSERT INTO sales.ledger SELECT 3; INSERT INTO remote.db.app.ledger VALUES(4);";
    let h = setup("sqlserver", sql);
    assert_eq!(
        hints(&h).await,
        [
            "local_value:",
            "other_value:",
            "sales_value:",
            "remote_value:"
        ]
    );
}
#[tokio::test]
async fn default_and_bare_insert_sources_require_namespace_proof() {
    for (effective, complete, primary, expected) in [
        (Some("sales"), false, true, Some("sales_value:")),
        (Some("sales"), true, false, Some("dbo_value:")),
        (Some("sales"), false, false, None),
        (None, true, true, None),
    ] {
        let h = setup("sqlserver", "INSERT INTO db..ledger VALUES(1);");
        context(&h, effective, complete, primary);
        assert_eq!(
            hints(&h).await,
            expected.into_iter().map(str::to_string).collect::<Vec<_>>()
        );
        if effective.is_some() {
            let h = setup("sqlserver", "INSERT INTO ledger VALUES(1);");
            context(&h, effective, complete, primary);
            assert_eq!(
                hints(&h).await,
                expected.into_iter().map(str::to_string).collect::<Vec<_>>()
            );
        }
    }
}
#[tokio::test]
async fn explicit_columns_work_without_any_physical_metadata() {
    let h = setup(
        "postgres",
        "INSERT INTO missing(a,b) VALUES (coalesce(1,2),json_build_object('x',1));",
    );
    h.service.inner().configured_file_schemas.clear();
    h.service.inner().schema_manager.clear();
    assert_eq!(hints(&h).await, ["a:", "b:"]);
}
#[tokio::test]
async fn metadata_refresh_cannot_reuse_previous_insert_labels() {
    let h = setup("mysql", "INSERT INTO other.app.ledger VALUES(1);");
    assert_eq!(hints(&h).await, ["other_value:"]);
    let manager = &h.service.inner().schema_manager;
    for id in manager.list_ids() {
        let mut s = manager.get(id).unwrap();
        if s.catalog.as_deref() == Some("other") {
            s.tables[0].columns[0].name = "updated".into();
            manager.update(id, s);
        }
    }
    assert_eq!(hints(&h).await, ["updated:"]);
}
#[tokio::test]
async fn insert_lookup_budget_keeps_cached_hits_and_explicit_columns() {
    let mut sql = (0..65)
        .map(|i| format!("INSERT INTO t{i} VALUES(1);"))
        .collect::<String>();
    sql.push_str("INSERT INTO t0 VALUES(1); INSERT INTO missing(explicit) VALUES(1);");
    let h = setup("mysql", &sql);
    let service = h.service.inner();
    let id = *service.configured_file_schemas.get(h.uri.as_str()).unwrap();
    let mut schema = service.schema_manager.get(id).unwrap();
    schema.tables = (0..65)
        .map(|i| table(&format!("t{i}"), &format!("c{i}")))
        .collect();
    service.schema_manager.update(id, schema);
    let labels = hints(&h).await;
    assert_eq!(labels.len(), 66);
    assert!(!labels.contains(&"c64:".into()));
    assert_eq!(&labels[64..], ["c0:", "explicit:"]);
}
#[tokio::test]
async fn ambiguity_inspection_uses_both_catalogs_and_local_projections() {
    for sql in ["SELECT shared FROM db.app.collision a JOIN other.app.collision b ON 1=1;","WITH q AS(SELECT shared FROM other.app.collision) SELECT shared FROM q JOIN db.app.collision b ON 1=1;","SELECT shared FROM other.app.collision a,other.app.collision b;"]{
        assert_eq!(ambiguity(&setup("sqlserver",sql)).await,1,"{sql}");
    }
    for sql in [
        "SELECT a.shared FROM db.app.collision a JOIN other.app.collision b ON 1=1;",
        "SELECT shared FROM db.app.collision a JOIN missing.app.collision b ON 1=1;",
    ] {
        assert_eq!(ambiguity(&setup("sqlserver", sql)).await, 0, "{sql}");
    }
}
#[tokio::test]
async fn ambiguity_refresh_and_empty_active_schema_still_use_exact_sources() {
    let h = setup(
        "sqlserver",
        "SELECT shared FROM db.app.collision a JOIN other.app.collision b ON 1=1;",
    );
    assert_eq!(ambiguity(&h).await, 1);
    let service = h.service.inner();
    for id in service.schema_manager.list_ids() {
        let mut s = service.schema_manager.get(id).unwrap();
        if s.catalog.as_deref() == Some("other") {
            s.tables[1].columns[0].name = "distinct".into();
            service.schema_manager.update(id, s);
        }
    }
    assert_eq!(ambiguity(&h).await, 0);
    let h = setup(
        "sqlserver",
        "SELECT shared FROM other.app.collision a JOIN other.app.collision b ON 1=1;",
    );
    let service = h.service.inner();
    let id = *service.configured_file_schemas.get(h.uri.as_str()).unwrap();
    let mut active = service.schema_manager.get(id).unwrap();
    active.tables.clear();
    service.schema_manager.update(id, active);
    assert_eq!(ambiguity(&h).await, 1);
}
#[tokio::test]
async fn schema_scoped_hints_and_ambiguity_work_across_sql_profiles() {
    for profile in [
        "postgres",
        "mysql",
        "sqlite",
        "hive",
        "clickhouse",
        "sqlserver",
    ] {
        assert_eq!(
            hints(&setup(profile, "INSERT INTO sales.ledger SELECT 1;")).await,
            ["sales_value:"],
            "{profile}"
        );
        assert_eq!(
            ambiguity(&setup(
                profile,
                "SELECT shared FROM app.collision a JOIN sales.collision b ON 1=1;"
            ))
            .await,
            1,
            "{profile}"
        );
    }
}
