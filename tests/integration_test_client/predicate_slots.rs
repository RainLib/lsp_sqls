use super::*;
use super::{
    execute_values::{complete, offset},
    joined_tables::position,
};

#[tokio::test]
async fn is_predicates_use_current_types_and_live_version_capabilities() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://predicate-version/live.mysql.sql";
    let sql = "SELECT * FROM jobs WHERE amount IS ";
    configure(&mut p, uri, "postgres").await;
    p.open(uri, sql).await;
    let items = complete(&mut p, uri, position(sql, sql.len())).await;
    assert!(!items
        .iter()
        .any(|item| ["TRUE", "FALSE", "UNKNOWN"]
            .contains(&item["label"].as_str().unwrap_or_default())));
    for (profile, version, allowed) in [
        ("sqlserver", Some("15.0.4382"), false),
        ("sqlserver", Some("16.0.4195"), true),
        ("sqlserver", None, false),
        ("clickhouse", Some("25.9.1"), false),
        ("clickhouse", Some("25.10.1"), true),
    ] {
        configure_version(&mut p, uri, profile, version).await;
        let items = complete(&mut p, uri, position(sql, sql.len())).await;
        assert_eq!(
            items
                .iter()
                .any(|item| item["label"] == "NOT DISTINCT FROM"),
            allowed,
            "{profile} {version:?}: {items:?}"
        );
    }
    p.stop().await;
}

#[tokio::test]
async fn document_is_keywords_use_local_types_and_request_preferences_only() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://predicate-local/isolated.mysql.sql";
    configure(&mut p, uri, "sqlserver").await;
    let marked =
        "/*😀*/ WITH q AS(SELECT CAST(1 AS INTEGER) AS flag) SELECT * FROM q WHERE flag IS NU|ll;";
    let at = marked.find('|').unwrap();
    let sql = marked.replace('|', "");
    p.open(uri, &sql).await;
    let empty_at = sql.rfind("NUll").unwrap();
    let unfiltered=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":position(&sql,empty_at),"preferences":{"keywordCase":"lower","functionCase":"lower","tableAlias":"none"}})).await;
    assert!(
        !unfiltered["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| ["true", "false", "unknown"]
                .contains(&item["label"].as_str().unwrap_or_default())),
        "local INTEGER output must not borrow the configured catalog's Boolean field: {unfiltered}"
    );
    let result=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":position(&sql,at),"preferences":{"keywordCase":"lower","functionCase":"lower","tableAlias":"none"}})).await;
    let items = result["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["label"], "null");
    assert!(items[0].get("data").is_none());
    let edit = &items[0]["textEdit"];
    let mut accepted = sql.clone();
    accepted.replace_range(
        offset(&sql, &edit["range"]["start"])..offset(&sql, &edit["range"]["end"]),
        edit["newText"].as_str().unwrap(),
    );
    assert!(accepted.ends_with("flag IS null;"));
    assert!(!result.to_string().contains("secret"));
    p.stop().await;
}

async fn configure(p: &mut LspProcess, uri: &str, profile: &str) {
    configure_version(p, uri, profile, None).await;
}
async fn configure_version(p: &mut LspProcess, uri: &str, profile: &str, version: Option<&str>) {
    p.notify("workspace/didChangeConfiguration",json!({"settings":{
  "schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","server_version":version,"tables":[{"name":"jobs","columns":[{"name":"flag","data_type":"boolean","nullable":true},{"name":"amount","data_type":"integer","nullable":true}]},{"name":"unrelated","columns":[{"name":"secret","data_type":"text","nullable":false}]}],"functions":[]}],
  "fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):profile},"completionPreferences":{"keywordCase":"upper"}
 }})).await;
}

#[tokio::test]
async fn is_keyword_slots_keep_valid_profile_syntax_without_unrelated_candidates() {
    let mut p = LspProcess::start().await;
    for (index, (profile, marked, expected, absent)) in [
        (
            "sqlserver",
            "SELECT * FROM jobs WHERE flag IS |;",
            vec!["NULL", "NOT NULL"],
            vec!["TRUE", "FALSE", "CURRENT_DATE", "CURRENT_TIMESTAMP"],
        ),
        (
            "postgres",
            "SELECT * FROM jobs WHERE flag IS NOT |;",
            vec!["NULL", "TRUE", "FALSE", "UNKNOWN", "DISTINCT FROM"],
            vec!["NOT NULL", "CURRENT_DATE", "CURRENT_TIMESTAMP"],
        ),
        (
            "mysql",
            "SELECT * FROM jobs WHERE flag IS |;",
            vec!["NULL", "NOT NULL", "TRUE", "FALSE", "UNKNOWN"],
            vec!["DISTINCT FROM", "CURRENT_DATE"],
        ),
        (
            "hive",
            "SELECT * FROM jobs WHERE flag IS |;",
            vec!["NULL", "NOT NULL", "TRUE", "FALSE"],
            vec!["UNKNOWN", "CURRENT_DATE"],
        ),
        (
            "clickhouse",
            "SELECT * FROM jobs WHERE amount IS NOT |;",
            vec!["NULL"],
            vec!["TRUE", "FALSE", "UNKNOWN", "CURRENT_DATE"],
        ),
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://predicate-slots/{index}.mysql.sql");
        configure(&mut p, &uri, profile).await;
        let at = marked.find('|').unwrap();
        let sql = marked.replace('|', "");
        p.open(&uri, &sql).await;
        let items = complete(&mut p, &uri, position(&sql, at)).await;
        for label in expected {
            assert!(
                items.iter().any(|item| item["label"] == *label),
                "{profile}: missing {label}: {items:?}"
            );
        }
        assert!(items.iter().all(|item|item["kind"]==14),"{profile}: a keyword-only IS grammar slot must not contain fields/functions/relations: {items:?}");
        for label in absent {
            assert!(
                !items.iter().any(|item| item["label"] == *label),
                "{profile}: invalid {label}: {items:?}"
            );
        }
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}

#[tokio::test]
async fn is_keyword_edits_preserve_completed_keywords_comments_unicode_and_suffixes() {
    let mut p = LspProcess::start().await;
    for (index, (marked, label, expected)) in [
        (
            "/*😀*/ SELECT * FROM jobs WHERE flag IS|;",
            "NULL",
            "/*😀*/ SELECT * FROM jobs WHERE flag IS NULL;",
        ),
        (
            "/*😀*/ SELECT * FROM jobs WHERE flag IS NOT|;",
            "NULL",
            "/*😀*/ SELECT * FROM jobs WHERE flag IS NOT NULL;",
        ),
        (
            "SELECT * FROM jobs WHERE flag IS |Null;",
            "NULL",
            "SELECT * FROM jobs WHERE flag IS NULL;",
        ),
        (
            "SELECT * FROM jobs WHERE flag IS /*😀 FROM*/ NU|ll;",
            "NULL",
            "SELECT * FROM jobs WHERE flag IS /*😀 FROM*/ NULL;",
        ),
        (
            "SELECT * FROM jobs WHERE flag IS NOT DISTINCT|;",
            "FROM",
            "SELECT * FROM jobs WHERE flag IS NOT DISTINCT FROM;",
        ),
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://predicate-edits/{index}.mysql.sql");
        configure(&mut p, &uri, "postgres").await;
        let at = marked.find('|').unwrap();
        let sql = marked.replace('|', "");
        p.open(&uri, &sql).await;
        let items = complete(&mut p, &uri, position(&sql, at)).await;
        let item = items
            .iter()
            .find(|item| item["label"] == *label)
            .unwrap_or_else(|| panic!("{marked}: {items:?}"));
        let edit = &item["textEdit"];
        let from = offset(&sql, &edit["range"]["start"]);
        let to = offset(&sql, &edit["range"]["end"]);
        let mut accepted = sql.clone();
        accepted.replace_range(from..to, edit["newText"].as_str().unwrap());
        assert_eq!(&accepted, expected);
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}

#[tokio::test]
async fn distinct_from_rhs_is_an_expression_scope_and_preserves_document_only_types() {
    let mut p = LspProcess::start().await;
    for (index, profile) in ["postgres", "duckdb", "sqlite", "sqlserver"]
        .iter()
        .enumerate()
    {
        let uri = format!("oxide://distinct-operands/{index}.mysql.sql");
        configure(&mut p, &uri, profile).await;
        let marked="WITH q AS(SELECT CAST(1 AS INTEGER) AS local_amount) SELECT * FROM q WHERE q.local_amount IS NOT DISTINCT FROM q.loc|Suffix;";
        let at = marked.find('|').unwrap();
        let sql = marked.replace('|', "");
        p.open(&uri, &sql).await;
        for document_only in [false, true] {
            let items = if document_only {
                p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(&sql,at),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await["items"].as_array().unwrap().clone()
            } else {
                complete(&mut p, &uri, position(&sql, at)).await
            };
            assert!(
                items
                    .iter()
                    .any(|item| item["label"] == "local_amount" && item["kind"] == 5),
                "{profile} document_only={document_only}: {items:?}"
            );
            assert!(!items.iter().any(|item| item["label"] == "secret"));
        }
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}
