use super::*;

#[tokio::test]
async fn sqlserver_search_rowsets_offer_key_and_rank_without_table_columns() {
    for function in ["CONTAINSTABLE", "FREETEXTTABLE"] {
        for tail in ["", ", 25", ", LANGUAGE 1033, 25"] {
            let items=candidates("sqlserver", &format!("SELECT ft.<cursor> FROM {function}(users, (name,email), N'oxide OR studio'{tail}) AS ft")).await;
            for name in ["KEY", "RANK"] {
                assert!(
                    items
                        .iter()
                        .any(|item| item.label.eq_ignore_ascii_case(name)),
                    "{function}{tail}: {items:?}"
                );
            }
            let rank = items
                .iter()
                .find(|item| item.label.eq_ignore_ascii_case("RANK"))
                .unwrap();
            assert!(
                rank.detail
                    .as_ref()
                    .is_some_and(|detail| detail.contains("int")),
                "{rank:?}"
            );
            assert!(!items
                .iter()
                .any(|item| matches!(item.label.as_str(), "name" | "email" | "audit_secret")));
        }
    }
}

#[tokio::test]
async fn sqlserver_rank_types_propagate_through_cte_and_join() {
    for function in ["CONTAINSTABLE", "FREETEXTTABLE"] {
        let harness=configured("sqlserver",&format!("WITH hits(document_key,relevance) AS (SELECT ft.[KEY],ft.RANK FROM {function}(users,*,N'NEAR((oxide,studio),5,TRUE)',LANGUAGE 1033,50) ft) SELECT h.re<cursor>levance FROM hits h JOIN users u ON u.id=h.document_key ORDER BY h.relevance DESC"));
        let items = complete_harness(&harness).await;
        let item = items
            .iter()
            .find(|item| item.label == "relevance")
            .expect("rank via CTE");
        assert!(
            item.detail
                .as_ref()
                .is_some_and(|detail| detail.contains("int")),
            "{item:?}"
        );
        let hover=harness.service.inner().hover(serde_json::from_value(serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position})).unwrap()).await.unwrap().expect("rank hover");
        assert!(
            serde_json::to_string(&hover).unwrap().contains("int"),
            "{hover:?}"
        );
    }
}

#[tokio::test]
async fn vendor_search_functions_appear_only_in_suitable_contexts() {
    for (dialect, sql, name) in [
        (
            "sqlserver",
            "SELECT * FROM contains<cursor>",
            "CONTAINSTABLE",
        ),
        ("sqlserver", "SELECT * FROM free<cursor>", "FREETEXTTABLE"),
        (
            "sqlserver",
            "SELECT * FROM users u WHERE contains<cursor>",
            "CONTAINS",
        ),
        ("oracle", "SELECT sco<cursor> FROM users u", "SCORE"),
        (
            "oracle",
            "SELECT u.id FROM users u WHERE contains<cursor>",
            "CONTAINS",
        ),
    ] {
        let items = candidates(dialect, sql).await;
        assert!(
            items
                .iter()
                .any(|item| item.label.eq_ignore_ascii_case(name)),
            "{dialect}: {sql}: {items:?}"
        );
    }
    let items = candidates("sqlserver", "SELECT contains<cursor> FROM users u").await;
    assert!(!items
        .iter()
        .any(|item| item.label.eq_ignore_ascii_case("CONTAINSTABLE")));
}

#[tokio::test]
async fn oracle_text_score_keeps_number_type_through_derived_query() {
    let harness=configured("oracle","SELECT r.re<cursor>levance FROM (SELECT SCORE(7) AS relevance,u.id FROM users u WHERE CONTAINS(u.name,'NEAR((oxide,studio),5)',7)>0) r ORDER BY r.relevance DESC");
    let items = complete_harness(&harness).await;
    let item = items
        .iter()
        .find(|item| item.label == "relevance")
        .expect("oracle score");
    assert!(
        item.detail
            .as_ref()
            .is_some_and(|detail| detail.to_ascii_lowercase().contains("number")),
        "{item:?}"
    );
}

#[tokio::test]
async fn search_table_signatures_track_language_argument() {
    let harness = configured(
        "sqlserver",
        "SELECT ft.RANK FROM CONTAINSTABLE(users, name, N'oxide',<cursor>) ft",
    );
    let result = harness
        .service
        .inner()
        .signature_help(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .expect("rowset signature");
    assert_eq!(result.active_parameter, Some(3));
    assert!(result
        .signatures
        .iter()
        .any(|signature| signature.label.to_ascii_uppercase().contains("LANGUAGE")));
}

#[tokio::test]
async fn fulltext_rowset_aliases_and_unnamed_sources_keep_shapes() {
    for sql in [
        "SELECT RA<cursor>NK FROM CONTAINSTABLE(users,name,N'oxide') ORDER BY RANK DESC",
        "SELECT ft.ra<cursor>ting FROM FREETEXTTABLE(users,name,N'oxide') AS ft(document_id,rating)",
        "WITH h AS (SELECT ft.* FROM CONTAINSTABLE(users,name,N'oxide') ft(document_id,rating)) SELECT h.ra<cursor>ting FROM h",
        "SELECT second.ra<cursor>ting FROM CONTAINSTABLE(users,name,N'oxide') first(k,first_rank) JOIN CONTAINSTABLE(users,email,N'sql') second(k,rating) ON first.k=second.k",
    ] {
        let items=candidates("sqlserver",sql).await;
        let name=if sql.contains("RA<cursor>") {"RANK"} else {"rating"};
        let item=items.iter().find(|item|item.label.eq_ignore_ascii_case(name)).expect("fulltext rank output");
        assert!(item.detail.as_ref().is_some_and(|detail|detail.contains("int")),"{sql}: {item:?}");
    }
    let items = candidates(
        "sqlserver",
        "SELECT ft.<cursor> FROM CONTAINSTABLE(users,name,N'oxide') ft",
    )
    .await;
    let key = items
        .iter()
        .find(|item| item.label.eq_ignore_ascii_case("KEY"))
        .unwrap();
    assert!(
        key.detail
            .as_ref()
            .is_some_and(|detail| detail.contains("unknown")),
        "cannot assume KEY is primary key: {key:?}"
    );
}

#[tokio::test]
async fn vendor_search_does_not_invent_rowsets_or_scalar_predicate_types() {
    for (dialect, sql) in [
        (
            "postgres",
            "SELECT ft.<cursor> FROM CONTAINSTABLE(users,name,'oxide') ft",
        ),
        (
            "mysql",
            "SELECT ft.<cursor> FROM FREETEXTTABLE(users,name,'oxide') ft",
        ),
        (
            "sqlserver",
            "SELECT ft.<cursor> FROM app.CONTAINSTABLE(users,name,'oxide') ft",
        ),
        (
            "sqlserver",
            "SELECT ft.<cursor> FROM CONTAINSTABLE(users,name) ft",
        ),
        (
            "sqlserver",
            "SELECT ft.<cursor> FROM CONTAINSTABLE('users',name,'oxide') ft",
        ),
        (
            "sqlserver",
            "SELECT ft.<cursor> FROM CONTAINSTABLE(users,name,'oxide',10,20) ft",
        ),
    ] {
        let items = candidates(dialect, sql).await;
        assert!(
            !items
                .iter()
                .any(|item| item.label.eq_ignore_ascii_case("KEY")
                    || item.label.eq_ignore_ascii_case("RANK")),
            "{dialect}: {sql}: {items:?}"
        );
    }
    for sql in [
        "SELECT contains<cursor> FROM users u",
        "SELECT * FROM users u ORDER BY contains<cursor>",
    ] {
        assert!(!candidates("sqlserver", sql)
            .await
            .iter()
            .any(|item| item.label.eq_ignore_ascii_case("CONTAINS")));
    }
}

#[tokio::test]
async fn searched_case_and_language_variants_keep_search_candidates() {
    for sql in [
        "SELECT CASE WHEN contains<cursor> THEN 1 ELSE 0 END FROM users u",
        "SELECT * FROM users u WHERE free<cursor>",
        "SELECT * FROM users u WHERE NOT contains<cursor>",
        "SELECT * FROM users u JOIN orders o ON contains<cursor>",
    ] {
        let expected = if sql.contains("free<cursor>") {
            "FREETEXT"
        } else {
            "CONTAINS"
        };
        let items = candidates("sqlserver", sql).await;
        assert!(
            items
                .iter()
                .any(|item| item.label.eq_ignore_ascii_case(expected)),
            "{sql}: {items:?}"
        );
    }
    for args in [
        "[app].[users],(name,email),N'oxide',LANGUAGE\r\n1033,25",
        "users, /* columns */ (name, email),@query,LANGUAGE /* locale */ N'English',25",
        "users,name,@query,LANGUAGE 'English',25",
    ] {
        let items = candidates(
            "sqlserver",
            &format!("SELECT ft.ra<cursor>nk FROM CONTAINSTABLE({args}) ft"),
        )
        .await;
        assert!(
            items
                .iter()
                .any(|item| item.label.eq_ignore_ascii_case("RANK")),
            "{args}: {items:?}"
        );
    }
    for args in [
        "users,123,N'oxide'",
        "users,'name',N'oxide'",
        "users,LOWER(name),N'oxide'",
    ] {
        let items = candidates(
            "sqlserver",
            &format!("SELECT ft.<cursor> FROM CONTAINSTABLE({args}) ft"),
        )
        .await;
        assert!(
            !items
                .iter()
                .any(|item| item.label.eq_ignore_ascii_case("RANK")),
            "{args}: {items:?}"
        );
    }
}

#[tokio::test]
async fn search_rowset_shapes_refresh_with_the_configured_dialect() {
    let harness = configured(
        "sqlserver",
        "SELECT ft.<cursor> FROM CONTAINSTABLE(users,name,N'oxide') ft",
    );
    assert!(complete_harness(&harness)
        .await
        .iter()
        .any(|item| item.label == "RANK"));
    harness
        .service
        .inner()
        .configured_file_dialects
        .insert(harness.uri.to_string(), "postgres".into());
    assert!(!complete_harness(&harness)
        .await
        .iter()
        .any(|item| item.label == "RANK"));
    harness
        .service
        .inner()
        .configured_file_dialects
        .insert(harness.uri.to_string(), "sqlserver".into());
    assert!(complete_harness(&harness)
        .await
        .iter()
        .any(|item| item.label == "RANK"));
}
