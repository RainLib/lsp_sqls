use super::*;

fn setup(
    marked: &str,
    default_type: Option<&str>,
    complete: bool,
    effective: Option<&str>,
) -> Harness {
    let h = configured("sqlserver", marked);
    let service = h.service.inner();
    let id = *service.configured_file_schemas.get(h.uri.as_str()).unwrap();
    let mut dbo = service.schema_manager.get(id).unwrap();
    dbo.catalog = Some("db".into());
    dbo.database = "dbo".into();
    dbo.tables = vec![Table {
        name: "ledger".into(),
        columns: vec![Column {
            name: "amount".into(),
            data_type: "decimal".into(),
            comment: Some("Revenue dbo".into()),
            ..Default::default()
        }],
        ..Default::default()
    }];
    dbo.namespace = effective.map(|s| {
        serde_json::from_value(serde_json::json!({"default_schema":s,"tables_complete":true}))
            .unwrap()
    });
    let mut primary = dbo.clone();
    primary.id = SchemaId::new();
    primary.database = "sales".into();
    primary.namespace = effective.map(|s| {
        serde_json::from_value(serde_json::json!({"default_schema":s,"tables_complete":complete}))
            .unwrap()
    });
    primary.tables = default_type
        .map(|ty| {
            let mut table = dbo.tables[0].clone();
            table.columns[0].data_type = ty.into();
            table.columns[0].comment = Some("Revenue sales".into());
            vec![table]
        })
        .unwrap_or_default();
    service.schema_manager.update(id, dbo);
    service.schema_manager.register(primary);
    h
}

async fn assert_type(h: &Harness, expected: &str) {
    let items = complete_harness(h).await;
    let item = items
        .iter()
        .find(|i| i.label == "amount" || i.label == "net")
        .unwrap_or_else(|| panic!("{items:?}"));
    assert!(
        item.detail
            .as_deref()
            .unwrap_or_default()
            .contains(expected),
        "{items:?}"
    );
}

#[tokio::test]
async fn known_default_schema_precedes_dbo_in_direct_cte_and_derived_queries() {
    for sql in [
        "SELECT r.Revenue<cursor> FROM db..ledger r",
        "SELECT r.Revenue<cursor> FROM [db]..[ledger] r",
        "WITH q AS (SELECT amount AS net FROM db..ledger) SELECT q.net<cursor> FROM q",
        "SELECT q.net<cursor> FROM (SELECT amount AS net FROM db..ledger) q",
    ] {
        for complete in [false, true] {
            let h = setup(sql, Some("uuid"), complete, Some("sales"));
            assert_type(&h, "uuid").await;
        }
    }
}

#[tokio::test]
async fn complete_primary_absence_allows_dbo_fallback_in_direct_and_cte_queries() {
    for sql in [
        "SELECT r.Revenue<cursor> FROM db..ledger r",
        "WITH q AS (SELECT amount AS net FROM db..ledger) SELECT q.net<cursor> FROM q",
    ] {
        let h = setup(sql, None, true, Some("sales"));
        assert_type(&h, "decimal").await;
    }
}

#[tokio::test]
async fn unknown_incomplete_or_missing_primary_never_proves_dbo_fallback() {
    for (effective, complete) in [
        (None, true),
        (Some("sales"), false),
        (Some("missing"), true),
    ] {
        let h = setup(
            "SELECT r.Revenue<cursor> FROM db..ledger r",
            None,
            complete,
            effective,
        );
        assert!(!complete_harness(&h)
            .await
            .iter()
            .any(|i| i.kind == Some(CompletionItemKind::FIELD)));
        let h = setup(
            "WITH q AS (SELECT amount AS net FROM db..ledger) SELECT q.net<cursor> FROM q",
            None,
            complete,
            effective,
        );
        assert_type(&h, "unknown").await;
    }
}

#[tokio::test]
async fn explicit_dbo_reference_does_not_depend_on_default_schema_context() {
    let h = setup(
        "SELECT r.Revenue<cursor> FROM db.dbo.ledger r",
        Some("uuid"),
        false,
        None,
    );
    assert_type(&h, "decimal").await;
}

#[tokio::test]
async fn conflicting_session_contexts_fail_closed_and_cannot_use_active_schema() {
    let h = setup(
        "SELECT r.Revenue<cursor> FROM db..ledger r",
        Some("uuid"),
        true,
        Some("sales"),
    );
    let service = h.service.inner();
    for id in service.schema_manager.list_ids() {
        let mut s = service.schema_manager.get(id).unwrap();
        if s.database == "dbo" {
            s.namespace.as_mut().unwrap().default_schema = "dbo".into();
            service.schema_manager.update(id, s);
        }
    }
    assert!(!complete_harness(&h)
        .await
        .iter()
        .any(|i| i.kind == Some(CompletionItemKind::FIELD)));
}

#[tokio::test]
async fn omitted_schema_lookup_is_exact_for_server_and_catalog() {
    for (sql, server, catalog) in [
        (
            "SELECT r.Revenue<cursor> FROM remote.db..ledger r",
            Some("remote"),
            "db",
        ),
        (
            "SELECT r.Revenue<cursor> FROM remote.db..ledger r",
            Some("other"),
            "db",
        ),
        (
            "SELECT r.Revenue<cursor> FROM remote.other..ledger r",
            Some("remote"),
            "db",
        ),
        (
            "SELECT r.Revenue<cursor> FROM remote...ledger r",
            Some("remote"),
            "db",
        ),
    ] {
        let h = setup(sql, Some("uuid"), true, Some("sales"));
        let manager = &h.service.inner().schema_manager;
        for id in manager.list_ids() {
            let mut s = manager.get(id).unwrap();
            s.server = server.map(str::to_string);
            s.catalog = Some(catalog.into());
            manager.update(id, s);
        }
        if sql.contains("remote.db..") && server == Some("remote") {
            assert_type(&h, "uuid").await;
        } else {
            assert!(!complete_harness(&h)
                .await
                .iter()
                .any(|i| i.kind == Some(CompletionItemKind::FIELD)));
        }
    }
}

#[tokio::test]
async fn coverage_and_effective_schema_refresh_change_the_same_document() {
    let h = setup(
        "SELECT r.Revenue<cursor> FROM db..ledger r",
        None,
        false,
        Some("sales"),
    );
    let manager = &h.service.inner().schema_manager;
    assert!(!complete_harness(&h)
        .await
        .iter()
        .any(|i| i.kind == Some(CompletionItemKind::FIELD)));
    for id in manager.list_ids() {
        let mut s = manager.get(id).unwrap();
        s.namespace.as_mut().unwrap().tables_complete = true;
        manager.update(id, s);
    }
    assert_type(&h, "decimal").await;
    for id in manager.list_ids() {
        let mut s = manager.get(id).unwrap();
        s.namespace = None;
        manager.update(id, s);
    }
    assert!(!complete_harness(&h)
        .await
        .iter()
        .any(|i| i.kind == Some(CompletionItemKind::FIELD)));
}

#[test]
fn namespace_snapshot_cannot_deserialize_an_ephemeral_resolution_proof() {
    let context: crate::schema::NamespaceContext = serde_json::from_value(
        serde_json::json!({"default_schema":"sales","resolved_references":["db..ledger"]}),
    )
    .unwrap();
    assert!(!context.tables_complete);
    assert!(context.resolved_references.is_empty());
    assert!(serde_json::to_value(context)
        .unwrap()
        .get("resolved_references")
        .is_none());
}
