use super::*;

fn metadata(dialect: &str, marked: &str) -> Harness {
    let h = configured(dialect, marked);
    let server = h.service.inner();
    let id = *server.configured_file_schemas.get(h.uri.as_str()).unwrap();
    let mut schema = server.schema_manager.get(id).unwrap();
    schema.tables.push(Table {
        name: "ledger".into(),
        columns: [
            ("租金总额", "decimal", None),
            ("amount", "decimal", Some("营业收入 Revenue amount")),
            ("rev_exact", "int", None),
            ("rev", "bigint", None),
            ("中文VIP_金额2", "decimal", None),
        ]
        .into_iter()
        .map(|(name, ty, comment)| Column {
            name: name.into(),
            data_type: ty.into(),
            comment: comment.map(str::to_owned),
            ..Default::default()
        })
        .collect(),
        ..Default::default()
    });
    schema.tables.push(Table {
        name: "租金报表".into(),
        ..Default::default()
    });
    schema.tables.push(Table {
        name: "secret_archive".into(),
        columns: vec![Column {
            name: "secret".into(),
            data_type: "text".into(),
            comment: Some("Revenue 租金总额".into()),
            ..Default::default()
        }],
        ..Default::default()
    });
    server.schema_manager.update(id, schema);
    h
}
fn field<'a>(items: &'a [CompletionItem], name: &str) -> &'a CompletionItem {
    items
        .iter()
        .find(|item| item.kind == Some(CompletionItemKind::FIELD) && item.label == name)
        .unwrap_or_else(|| panic!("missing {name}: {items:?}"))
}

#[tokio::test]
async fn metadata_search_pinyin_prefix_subsequence_and_mixed_names_all_profiles() {
    for dialect in [
        "postgres",
        "mysql",
        "sqlserver",
        "sqlite",
        "hive",
        "clickhouse",
    ] {
        for (query, name) in [
            ("zj", "租金总额"),
            ("jze", "租金总额"),
            ("zwvipje2", "中文VIP_金额2"),
        ] {
            let h = metadata(dialect, &format!("SELECT {query}<cursor> FROM ledger"));
            let items = complete_harness(&h).await;
            let item = field(&items, name);
            assert!(
                item.detail.as_deref().unwrap().contains("(decimal)"),
                "{dialect}: {item:?}"
            );
            assert!(item.text_edit.is_some(), "{dialect}: missing edit {item:?}");
        }
    }
}

#[tokio::test]
async fn metadata_search_comments_keep_name_identity_and_name_priority() {
    for query in ["Revenue", "营业", "yysr", "rva"] {
        let h = metadata("mysql", &format!("SELECT {query}<cursor> FROM ledger"));
        let items = complete_harness(&h).await;
        let item = field(&items, "amount");
        assert!(
            item.filter_text
                .as_deref()
                .unwrap()
                .to_lowercase()
                .contains(&query.to_lowercase())
                || query == "rva"
        );
        assert!(!serde_json::to_string(&item.text_edit)
            .unwrap()
            .contains("Revenue"));
    }
    let h = metadata("mysql", "SELECT rev<cursor> FROM ledger");
    let items = complete_harness(&h).await;
    assert!(field(&items, "rev").sort_text < field(&items, "rev_exact").sort_text);
    assert!(field(&items, "rev_exact").sort_text < field(&items, "amount").sort_text);
}

#[tokio::test]
async fn metadata_search_qualified_members_and_unicode_full_suffix_edits() {
    for dialect in [
        "postgres",
        "mysql",
        "sqlserver",
        "sqlite",
        "hive",
        "clickhouse",
    ] {
        let h = metadata(
            dialect,
            "/* 😀 */\r\nSELECT l.zj<cursor>suffix FROM ledger l",
        );
        let items = complete_harness(&h).await;
        let item = field(&items, "租金总额");
        let CompletionTextEdit::Edit(edit) = item.text_edit.as_ref().unwrap() else {
            panic!("simple edit required")
        };
        assert_eq!(edit.range.start, Position::new(1, 9));
        assert_eq!(edit.range.end, Position::new(1, 17));
        assert!(edit.new_text.contains("租金总额"));
        assert!(!edit.new_text.contains("zj"));
    }
}

#[tokio::test]
async fn metadata_search_scope_noise_and_alias_shadowing_do_not_leak() {
    for marked in [
        "SELECT rev<cursor> FROM users",
        "SELECT l.rev<cursor> FROM users l WHERE EXISTS (SELECT 1 FROM ledger l)",
        "SELECT 1 FROM ledger; SELECT rev<cursor> FROM users",
        "SELECT 'rev<cursor>' FROM ledger",
        "SELECT 1 /* rev<cursor> */ FROM ledger",
        "SELECT rev<cursor> FROM users UNION SELECT amount FROM ledger",
    ] {
        let h = metadata("mysql", marked);
        let items = complete_harness(&h).await;
        assert!(
            !items
                .iter()
                .any(|i| i.label == "amount" || i.label == "secret"),
            "{marked}: {items:?}"
        );
    }
    let h = metadata("mysql", "SELECT l.rev<cursor> FROM ledger l");
    let items = complete_harness(&h).await;
    field(&items, "amount");
    assert!(!items.iter().any(|i| i.label == "secret"));
}

#[tokio::test]
async fn metadata_search_direct_projection_comments_survive_renaming_but_not_expressions() {
    for marked in [
        "WITH q AS(SELECT amount AS net FROM ledger) SELECT q.rev<cursor> FROM q",
        "WITH q(net) AS(SELECT amount FROM ledger) SELECT q.rev<cursor> FROM q",
        "SELECT q.rev<cursor> FROM (SELECT amount AS net FROM ledger) q",
        "WITH a AS(SELECT * FROM ledger), b AS(SELECT amount AS net FROM a) SELECT b.rev<cursor> FROM b",
    ] {let h=metadata("mysql",marked);field(&complete_harness(&h).await,"net");}
    let h = metadata(
        "mysql",
        "WITH q AS(SELECT amount+1 AS net FROM ledger) SELECT q.rev<cursor> FROM q",
    );
    // Qualified completion may return the whole bound row for client filtering.
    // A computed output must carry neither an inherited comment nor search text.
    let items = complete_harness(&h).await;
    let item = field(&items, "net");
    assert!(item.data.as_ref().unwrap()["oxide"]["searchComment"].is_null());
    assert!(!item.filter_text.as_deref().unwrap().contains("Revenue"));
}

#[tokio::test]
async fn metadata_search_refresh_removes_old_comments_names_and_schemas_without_sql_change() {
    let h = metadata("mysql", "SELECT rev<cursor> FROM ledger");
    field(&complete_harness(&h).await, "amount");
    let server = h.service.inner();
    let id = *server.configured_file_schemas.get(h.uri.as_str()).unwrap();
    let mut schema = server.schema_manager.get(id).unwrap();
    let table = schema
        .tables
        .iter_mut()
        .find(|t| t.name == "ledger")
        .unwrap();
    table
        .columns
        .iter_mut()
        .find(|c| c.name == "amount")
        .unwrap()
        .comment = Some("other".into());
    server.schema_manager.update(id, schema);
    assert!(!complete_harness(&h)
        .await
        .iter()
        .any(|i| i.label == "amount"));
    server.schema_manager.remove(id);
    assert!(!complete_harness(&h)
        .await
        .iter()
        .any(|i| i.label == "amount"));
}

#[tokio::test]
async fn metadata_search_pinyin_relation_entry_stays_out_of_field_context() {
    let h = metadata("mysql", "SELECT * FROM zj<cursor>");
    let items = complete_harness(&h).await;
    assert!(
        items
            .iter()
            .any(|i| i.label == "租金报表" && i.kind == Some(CompletionItemKind::CLASS)),
        "{items:?}"
    );
    assert!(!items
        .iter()
        .any(|i| i.kind == Some(CompletionItemKind::FIELD)));
}

#[tokio::test]
async fn metadata_search_qualify_and_correlated_outer_inputs_keep_current_bindings() {
    for (dialect,marked,name) in [
        ("mysql","SELECT * FROM ledger a WHERE EXISTS (SELECT a.rev<cursor> FROM users)","amount"),
        ("mysql","SELECT * FROM ledger a WHERE EXISTS (SELECT rev<cursor> FROM users)","a.amount"),
        ("clickhouse","SELECT row_number() OVER (ORDER BY id) AS `收入` FROM users QUALIFY sr<cursor>>0","收入"),
        ("clickhouse","SELECT amount AS net,row_number() OVER (ORDER BY amount) AS rn FROM ledger QUALIFY rev<cursor>>0","net"),
    ] {let h=metadata(dialect,marked);field(&complete_harness(&h).await,name);}
}

#[tokio::test]
async fn metadata_search_capped_candidates_keep_late_exact_names_and_requery() {
    let mut h = metadata("mysql", "SELECT rev<cursor> FROM ledger");
    let server = h.service.inner();
    let id = *server.configured_file_schemas.get(h.uri.as_str()).unwrap();
    let mut schema = server.schema_manager.get(id).unwrap();
    let table = schema
        .tables
        .iter_mut()
        .find(|t| t.name == "ledger")
        .unwrap();
    table.columns = (0..300)
        .map(|i| Column {
            name: format!("v{i:03}"),
            data_type: "int".into(),
            comment: Some("Revenue".into()),
            ..Default::default()
        })
        .collect();
    table.columns.push(Column {
        name: "rev".into(),
        data_type: "bigint".into(),
        ..Default::default()
    });
    server.schema_manager.update(id, schema);
    let response = server
        .completion(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    let CompletionResponse::List(list) = response else {
        panic!("incomplete list required")
    };
    assert!(list.is_incomplete);
    assert_eq!(
        list.items
            .iter()
            .filter(|i| i.kind == Some(CompletionItemKind::FIELD))
            .count(),
        256
    );
    field(&list.items, "rev");
    server
        .document_manager
        .update(h.uri.to_string(), "SELECT v299 FROM ledger".into(), 2);
    h.position = Position::new(0, 11);
    let items = complete_harness(&h).await;
    field(&items, "v299");
    assert_eq!(
        items
            .iter()
            .filter(|i| i.kind == Some(CompletionItemKind::FIELD))
            .count(),
        1
    );
}
