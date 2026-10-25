use super::*;

#[tokio::test]
async fn advanced_search_member_candidates_keep_alias_scope() {
    for (dialect,sql,expected) in [
        ("mysql","SELECT MATCH(u.na<cursor>me,u.email) AGAINST ('+oxide -legacy' IN BOOLEAN MODE) FROM users u","name"),
        ("mysql","SELECT u.id FROM users u WHERE MATCH(u.name,u.email) AGAINST ('oxide' WITH QUERY EXPANSION) > 0 AND u.em<cursor>ail IS NOT NULL","email"),
        ("postgres","SELECT u.id FROM users u WHERE to_tsvector('english',u.na<cursor>me) @@ websearch_to_tsquery('english','oxide OR studio')","name"),
        ("postgres","SELECT u.id FROM users u WHERE u.payload::jsonb @? '$.tags[*] ? (@ == \"sql\")' AND u.em<cursor>ail ~* 'oxide'","email"),
        ("mysql","SELECT u.id FROM users u WHERE REGEXP_LIKE(u.name,'^oxide','i') AND JSON_CONTAINS(u.payload,'\"sql\"','$.tags') AND u.em<cursor>ail IS NOT NULL","email"),
    ] {
        let items=candidates(dialect,sql).await;
        assert!(items.iter().any(|item|item.label==expected),"{dialect}: {sql}: {items:?}");
        assert!(!items.iter().any(|item|item.label=="audit_secret"),"unrelated search source leaked");
    }
}

#[tokio::test]
async fn search_functions_appear_in_their_own_dialect_lists() {
    for (dialect, prefix, expected) in [
        ("postgres", "to_ts", "TO_TSVECTOR"),
        ("postgres", "websearch_to", "WEBSEARCH_TO_TSQUERY"),
        ("postgres", "ts_ra", "TS_RANK"),
        ("mysql", "regexp_li", "REGEXP_LIKE"),
    ] {
        let items = candidates(dialect, &format!("SELECT {prefix}<cursor> FROM users u")).await;
        assert!(
            items
                .iter()
                .any(|item| item.label.eq_ignore_ascii_case(expected)),
            "{dialect}: {expected}: {items:?}"
        );
    }
    let items = candidates("mysql", "SELECT websearch_to<cursor> FROM users u").await;
    assert!(!items
        .iter()
        .any(|item| item.label.eq_ignore_ascii_case("WEBSEARCH_TO_TSQUERY")));
}

#[tokio::test]
async fn search_projection_candidates_and_hover_keep_types() {
    for (dialect, expr, expected) in [
        ("postgres", "to_tsvector('english',u.name)", "tsvector"),
        (
            "postgres",
            "websearch_to_tsquery('english',u.name)",
            "tsquery",
        ),
        (
            "postgres",
            "ts_rank(to_tsvector(u.name),plainto_tsquery('oxide'))",
            "real",
        ),
        (
            "postgres",
            "to_tsvector(u.name) @@ to_tsquery('oxide:*')",
            "boolean",
        ),
        (
            "postgres",
            "jsonb_path_exists(u.payload::jsonb,'$.tags[*] ? (@ == \"sql\")')",
            "boolean",
        ),
        (
            "mysql",
            "MATCH(u.name,u.email) AGAINST ('+oxide -legacy' IN BOOLEAN MODE)",
            "double",
        ),
        ("mysql", "REGEXP_LIKE(u.name,'^oxide','i')", "integer"),
    ] {
        let harness=configured(dialect,&format!("WITH searched(result) AS (SELECT {expr} FROM users u) SELECT s.re<cursor>sult FROM searched s"));
        let items = complete_harness(&harness).await;
        let item = items
            .iter()
            .find(|item| item.label == "result")
            .expect("search result candidate");
        assert!(
            item.detail
                .as_ref()
                .is_some_and(|detail| detail.to_ascii_lowercase().contains(expected)),
            "{dialect}: {expr}: {item:?}"
        );
        let hover=harness.service.inner().hover(serde_json::from_value(serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position})).unwrap()).await.unwrap().expect("search result hover");
        assert!(
            serde_json::to_string(&hover)
                .unwrap()
                .to_ascii_lowercase()
                .contains(expected),
            "{dialect}: {hover:?}"
        );
    }
}

#[tokio::test]
async fn search_signature_help_tracks_nested_call_and_active_argument() {
    let harness = configured(
        "postgres",
        "SELECT ts_rank(to_tsvector(u.name),websearch_to_tsquery('english',<cursor>)) FROM users u",
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
        .expect("search signature");
    assert!(result.signatures.iter().any(|signature| signature
        .label
        .to_ascii_lowercase()
        .starts_with("websearch_to_tsquery")));
    assert_eq!(result.active_parameter, Some(1));
}

#[tokio::test]
async fn mysql_match_completion_inserts_against_syntax() {
    let items = candidates("mysql", "SELECT mat<cursor> FROM users u").await;
    let item = items
        .iter()
        .find(|item| item.label.eq_ignore_ascii_case("MATCH"))
        .expect("MATCH search completion");
    assert!(
        item.insert_text
            .as_ref()
            .is_some_and(|text| text.contains("AGAINST") && text.contains("BOOLEAN MODE")),
        "{item:?}"
    );
}

#[tokio::test]
async fn search_version_refresh_recomputes_same_document_candidates() {
    for (dialect, marked, name, versions) in [
        (
            "postgres",
            "SELECT websearch_to<cursor> FROM users u",
            "WEBSEARCH_TO_TSQUERY",
            ["10.0", "11.0", "10.0"],
        ),
        (
            "mysql",
            "SELECT json_cont<cursor> FROM users u",
            "JSON_CONTAINS",
            ["5.6", "5.7", "5.6"],
        ),
    ] {
        let harness = configured(dialect, marked);
        let server = harness.service.inner();
        let id = *server
            .configured_file_schemas
            .get(harness.uri.as_str())
            .unwrap();
        for (index, version) in versions.into_iter().enumerate() {
            let mut schema = server.schema_manager.get(id).unwrap();
            schema.server_version = Some(version.into());
            assert!(server.schema_manager.update(id, schema));
            let items = complete_harness(&harness).await;
            assert_eq!(
                items
                    .iter()
                    .any(|item| item.label.eq_ignore_ascii_case(name)),
                index == 1,
                "{dialect} {version}: {items:?}"
            );
        }
    }
}

#[tokio::test]
async fn search_strings_and_comments_do_not_offer_sql_candidates() {
    for (dialect, sql) in [
        (
            "mysql",
            "SELECT MATCH(u.name) AGAINST ('u.na<cursor>me' IN BOOLEAN MODE) FROM users u",
        ),
        (
            "postgres",
            "SELECT websearch_to_tsquery('u.na<cursor>me') FROM users u",
        ),
        ("postgres", "SELECT u.id FROM users u /* u.na<cursor>me */"),
    ] {
        let items = candidates(dialect, sql).await;
        assert!(
            !items
                .iter()
                .any(|item| item.label == "name" || item.label.eq_ignore_ascii_case("TO_TSVECTOR")),
            "{dialect}: {items:?}"
        );
    }
}

#[tokio::test]
async fn weighted_search_pipeline_preserves_rank_across_cte_window_and_derived() {
    let sql="WITH search_input AS (SELECT websearch_to_tsquery('english','oxide OR sql -legacy') AS query), documents AS (SELECT u.id, setweight(to_tsvector('english',COALESCE(u.name,'')),'A') || setweight(to_tsvector('simple',COALESCE(u.email,'')),'B') AS document FROM users u), scored AS (SELECT d.id, ts_rank_cd(d.document,q.query,32) AS score, row_number() OVER (ORDER BY ts_rank(d.document,q.query) DESC) AS position FROM documents d CROSS JOIN search_input q WHERE d.document @@ q.query) SELECT r.sc<cursor>ore FROM (SELECT * FROM scored WHERE score > 0.01 AND NOT (position > 50)) r ORDER BY r.score DESC LIMIT 20";
    let harness = configured("postgres", sql);
    let items = complete_harness(&harness).await;
    let item = items
        .iter()
        .find(|item| item.label == "score")
        .expect("pipeline score");
    assert!(
        item.detail
            .as_ref()
            .is_some_and(|detail| detail.contains("real")),
        "{item:?}"
    );
    assert!(!items.iter().any(|item| item.label == "audit_secret"));
    let hover = harness
        .service
        .inner()
        .hover(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .expect("pipeline hover");
    assert!(
        serde_json::to_string(&hover).unwrap().contains("real"),
        "{hover:?}"
    );
}
