use super::*;

fn setup(marked: &str, server_name: Option<&str>, ty: &str) -> Harness {
    let h = configured("sqlserver", marked);
    let server = h.service.inner();
    let id = *server.configured_file_schemas.get(h.uri.as_str()).unwrap();
    let mut schema = server.schema_manager.get(id).unwrap();
    schema.catalog = Some("db".into());
    schema.server = server_name.map(str::to_string);
    schema.tables = vec![Table {
        name: "ledger".into(),
        columns: vec![Column {
            name: "amount".into(),
            data_type: ty.into(),
            comment: Some("Revenue namespace".into()),
            ..Default::default()
        }],
        ..Default::default()
    }];
    server.schema_manager.update(id, schema);
    h
}

#[tokio::test]
async fn unknown_server_never_matches_a_local_homonym() {
    for sql in [
        "SELECT r.Rev<cursor> FROM remote.db.app.ledger r",
        "SELECT r.Rev<cursor> FROM [remote].[db].[app].[ledger] r",
        "WITH q AS (SELECT amount AS net FROM remote.db.app.ledger) SELECT q.Rev<cursor> FROM q",
        "SELECT q.Rev<cursor> FROM (SELECT amount AS net FROM remote.db.app.ledger) q",
    ] {
        let h = setup(sql, None, "decimal");
        let items = complete_harness(&h).await;
        for item in items
            .iter()
            .filter(|i| i.kind == Some(CompletionItemKind::FIELD))
        {
            assert_eq!(item.label, "net", "{sql}: {item:?}");
            assert!(
                item.detail
                    .as_deref()
                    .unwrap_or_default()
                    .contains("unknown"),
                "{sql}: {item:?}"
            );
            assert!(
                item.data
                    .as_ref()
                    .and_then(|data| data.get("oxide"))
                    .and_then(|source| source.get("searchComment"))
                    .is_none_or(|comment| comment.is_null()),
                "{sql}: {item:?}"
            );
        }
    }
}

#[tokio::test]
async fn explicit_server_identity_preserves_direct_and_local_metadata() {
    for (sql,name) in [
        ("SELECT r.Rev<cursor> FROM remote.db.app.ledger r","amount"),
        ("WITH q AS (SELECT amount AS net FROM remote.db.app.ledger) SELECT q.Rev<cursor> FROM q","net"),
        ("SELECT q.Rev<cursor> FROM (SELECT amount AS net FROM remote.db.app.ledger) q","net"),
    ] {
        let h=setup(sql,Some("remote"),"nvarchar");let items=complete_harness(&h).await;
        let item=items.iter().find(|i|i.label==name).unwrap_or_else(||panic!("{sql}: {items:?}"));
        assert!(item.detail.as_deref().unwrap_or_default().contains("nvarchar"),"{sql}: {item:?}");
        if name=="amount"{assert_eq!(item.data.as_ref().unwrap()["oxide"]["server"],"remote");}
    }
}

#[tokio::test]
async fn server_and_catalog_are_both_required_when_supplied() {
    for sql in [
        "SELECT r.Rev<cursor> FROM other.db.app.ledger r",
        "SELECT r.Rev<cursor> FROM remote.other.app.ledger r",
        "SELECT r.Rev<cursor> FROM x.remote.db.app.ledger r",
    ] {
        let h = setup(sql, Some("remote"), "nvarchar");
        let items = complete_harness(&h).await;
        assert!(
            !items
                .iter()
                .any(|i| i.kind == Some(CompletionItemKind::FIELD)),
            "{sql}: {items:?}"
        );
    }
}

#[tokio::test]
async fn same_catalog_on_different_servers_never_uses_registration_order() {
    for (sql, expected) in [
        (
            "SELECT r.Rev<cursor> FROM remote.db.app.ledger r",
            "nvarchar",
        ),
        ("SELECT r.Rev<cursor> FROM db.app.ledger r", "decimal"),
        ("WITH q AS (SELECT amount AS amount FROM remote.db.app.ledger) SELECT q.Rev<cursor> FROM q", "nvarchar"),
        ("WITH q AS (SELECT r.amount AS amount FROM db.app.ledger l JOIN remote.db.app.ledger r ON 1=1) SELECT q.Rev<cursor> FROM q", "nvarchar"),
        ("WITH q AS (SELECT l.amount AS amount FROM db.app.ledger l JOIN remote.db.app.ledger r ON 1=1) SELECT q.Rev<cursor> FROM q", "decimal"),
        ("WITH q AS (SELECT amount AS amount FROM db.app.ledger l JOIN missing.db.app.ledger r ON 1=1) SELECT q.amount<cursor> FROM q", "unknown"),
    ] {
        let h = setup(sql, None, "decimal");
        let server = h.service.inner();
        let id = *server.configured_file_schemas.get(h.uri.as_str()).unwrap();
        let mut remote = server.schema_manager.get(id).unwrap();
        remote.id = SchemaId::new();
        remote.server = Some("remote".into());
        remote.tables[0].columns[0].data_type = "nvarchar".into();
        server.schema_manager.register(remote);
        let items = complete_harness(&h).await;
        let item = items
            .iter()
            .find(|i| i.label == "amount")
            .unwrap_or_else(|| panic!("{sql}: {items:?}"));
        assert!(
            item.detail
                .as_deref()
                .unwrap_or_default()
                .contains(expected),
            "{sql}: {item:?}"
        );
    }
}

#[tokio::test]
async fn server_identity_changes_refresh_the_same_sql() {
    let h = setup(
        "SELECT r.Rev<cursor> FROM remote.db.app.ledger r",
        None,
        "nvarchar",
    );
    let server = h.service.inner();
    let id = *server.configured_file_schemas.get(h.uri.as_str()).unwrap();
    assert!(!complete_harness(&h)
        .await
        .iter()
        .any(|i| i.label == "amount"));
    let mut schema = server.schema_manager.get(id).unwrap();
    schema.server = Some("remote".into());
    server.schema_manager.update(id, schema);
    assert!(complete_harness(&h)
        .await
        .iter()
        .any(|i| i.label == "amount"));
    let mut schema = server.schema_manager.get(id).unwrap();
    schema.server = Some("other".into());
    server.schema_manager.update(id, schema);
    assert!(!complete_harness(&h)
        .await
        .iter()
        .any(|i| i.label == "amount"));
}

#[test]
fn legacy_schema_json_keeps_server_optional_and_new_metadata_round_trips() {
    let mut schema: Schema = serde_json::from_value(
        serde_json::json!({"id":SchemaId::new(),"database":"app","tables":[],"functions":[]}),
    )
    .unwrap();
    assert!(schema.server.is_none());
    assert!(serde_json::to_value(&schema)
        .unwrap()
        .get("server")
        .is_none());
    schema.server = Some("remote".into());
    let copy: Schema = serde_json::from_value(serde_json::to_value(schema).unwrap()).unwrap();
    assert_eq!(copy.server.as_deref(), Some("remote"));
}
