use super::*;

#[tokio::test]
async fn pattern_escape_capability_is_explicit_versioned_and_independent_of_document_catalog_state()
{
    let mut p = LspProcess::start().await;
    for (index, (profile, version, expression, expected)) in [
        ("clickhouse", None, "'x' LIKE 'x'", None),
        ("clickhouse", Some("26.5.2"), "'x' LIKE 'x'", Some(false)),
        (
            "clickhouse",
            Some("26.6.1.123"),
            "'x' ILIKE 'x'",
            Some(true),
        ),
        ("clickhouse", Some("26.9.1"), "'x' SIMILAR TO 'x'", None),
        (
            "clickhouse",
            Some("26.10.1"),
            "'x' SIMILAR TO 'x'",
            Some(true),
        ),
        ("postgres", None, "'x' LIKE 'x'", Some(true)),
        ("postgres", None, "'x' ~ 'x'", Some(false)),
        ("duckdb", None, "'x' SIMILAR TO 'x'", Some(false)),
        ("hive", None, "'x' LIKE 'x'", Some(false)),
        ("sqlite", None, "'x' GLOB 'x'", Some(false)),
        ("sqlserver", None, "'x' LIKE 'x' ESCAPE NULL", Some(false)),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://pattern-capability/{index}.sql");
        let text = format!("/*😀*/\r\nSELECT 1 WHERE {expression} ");
        let token = format!("pattern-capability-{index}");
        configure(&mut p, &uri, profile, &token, version, "String").await;
        p.open(&uri, &text).await;
        let catalog = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, &text, text.len(), profile, &token),
            )
            .await;
        assert_eq!(
            catalog.get("patternEscape").and_then(Value::as_bool),
            expected,
            "{profile}: {version:?}: {expression}"
        );
        if expected.is_some() {
            assert_eq!(catalog["logicalContinuation"], true);
        }
        let doc = local(&mut p, &uri, &text, text.len(), profile).await;
        assert_eq!(
            doc.get("patternEscape").and_then(Value::as_bool),
            if profile == "clickhouse" {
                None
            } else {
                expected
            },
            "document {profile}: {expression}"
        );
        for response in [&catalog, &doc] {
            assert!(response.get("valueType").is_none());
            assert!(response.get("valueRole").is_none());
        }
    }
    p.stop().await;
}

#[tokio::test]
async fn nullable_and_dynamic_escapes_keep_explicit_logical_contracts_in_all_routes() {
    let mut p = LspProcess::start().await;
    for (index, (profile, expression)) in [
        ("postgres", "'x' LIKE 'x' ESCAPE NULL"),
        ("duckdb", "'x' LIKE 'x' ESCAPE NULL"),
        ("duckdb", "'x' LIKE 'x' ESCAPE SUBSTR('##',1,1)"),
        ("sqlite", "'x' LIKE 'x' ESCAPE NULL"),
        ("sqlserver", "'x' LIKE 'x' ESCAPE NULL"),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://nullable-pattern/{index}.sql");
        let text = format!("/*😀*/\r\nSELECT 1 WHERE {expression} ");
        configure(&mut p, &uri, profile, "nullable-pattern", None, "integer").await;
        p.open(&uri, &text).await;
        let catalog = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, &text, text.len(), profile, "nullable-pattern"),
            )
            .await;
        let doc = local(&mut p, &uri, &text, text.len(), profile).await;
        let standard = complete(&mut p, &uri, position(&text, text.len())).await;
        for response in [&catalog, &doc] {
            assert_eq!(
                response["logicalContinuation"], true,
                "{profile}: {expression}"
            );
            assert!(response.get("valueType").is_none());
            assert!(!has_escape(response));
        }
        for items in [
            catalog["items"].as_array().unwrap(),
            doc["items"].as_array().unwrap(),
            &standard,
        ] {
            assert!(items.iter().any(|item| item["label"] == "AND"));
        }
    }
    p.stop().await;
}
use super::{execute_values::complete, joined_tables::position};
async fn configure(
    p: &mut LspProcess,
    uri: &str,
    profile: &str,
    token: &str,
    version: Option<&str>,
    ty: &str,
) {
    p.notify("workspace/didChangeConfiguration",json!({"settings":{"oxideAnalysisToken":token,"schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","server_version":version,"tables":[],"functions":[{"name":"semantic_result","routine_type":"function","parameters":[],"return_type":ty}]}],"fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):profile},"completionPreferences":{"keywordCase":"upper"}}})).await;
    assert_eq!(
        p.request(
            "oxide/analysisConfiguration",
            json!({"oxideAnalysisToken":token})
        )
        .await["applied"],
        true
    );
}
fn params(uri: &str, text: &str, cursor: usize, profile: &str, token: &str) -> Value {
    json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(text,cursor),"oxideAnalysisToken":token,"context":{"triggerKind":1}})
}
async fn local(p: &mut LspProcess, uri: &str, text: &str, cursor: usize, profile: &str) -> Value {
    p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(text,cursor),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await
}
fn has_escape(response: &Value) -> bool {
    response["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["label"] == "ESCAPE")
}

#[tokio::test]
async fn matching_continuations_offer_exact_escape_edits_in_all_three_routes() {
    let mut p = LspProcess::start().await;
    for (index, (profile, operator)) in [
        ("postgres", "LIKE"),
        ("postgres", "NOT SIMILAR TO"),
        ("duckdb", "ILIKE"),
        ("mysql", "LIKE"),
        ("mariadb", "LIKE"),
        ("sqlite", "NOT LIKE"),
        ("sqlserver", "LIKE"),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://pattern-continuation/{index}.sql");
        let source = format!("/*😀*/\r\nSELECT 1 WHERE LOWER('oxide_1') {operator} 'oxide#_1' ");
        let prefix = format!("{source}E");
        let text = format!("{prefix}Suffix尾;");
        configure(
            &mut p,
            &uri,
            profile,
            "pattern-continuation",
            None,
            "integer",
        )
        .await;
        p.open(&uri, &text).await;
        let catalog = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, &text, prefix.len(), profile, "pattern-continuation"),
            )
            .await;
        let doc = local(&mut p, &uri, &text, prefix.len(), profile).await;
        let standard = complete(&mut p, &uri, position(&text, prefix.len())).await;
        for response in [&catalog, &doc] {
            assert_eq!(response["logicalContinuation"], true, "{profile}");
            assert!(response.get("valueType").is_none());
            assert!(response.get("valueRole").is_none());
        }
        for items in [
            catalog["items"].as_array().unwrap(),
            doc["items"].as_array().unwrap(),
            &standard,
        ] {
            let item = items
                .iter()
                .find(|item| item["label"] == "ESCAPE" && item["kind"] == 14)
                .unwrap_or_else(|| panic!("{profile}: {items:?}"));
            assert_eq!(item["textEdit"]["newText"], "ESCAPE ");
            assert_eq!(
                item["textEdit"]["range"]["start"],
                json!(position(&text, source.len()))
            );
            assert_eq!(
                item["textEdit"]["range"]["end"],
                json!(position(&text, text.len() - 1))
            );
        }
    }
    p.stop().await;
}

#[tokio::test]
async fn matching_continuation_versions_refresh_without_leaking_catalog_knowledge_to_document_mode()
{
    let mut p = LspProcess::start().await;
    for operator in ["LIKE", "SIMILAR TO"] {
        let uri = format!("oxide://pattern-version/{}.sql", operator.replace(' ', "-"));
        let text = format!("SELECT 1 WHERE 'x' {operator} 'x' ");
        p.open(&uri, &text).await;
        for (index, version) in [
            None,
            Some("26.5.2"),
            Some("26.6.1.123"),
            Some("26.9.1"),
            Some("26.10.1"),
        ]
        .into_iter()
        .enumerate()
        {
            let token = format!("pattern-version-{}-{index}", operator.replace(' ', "-"));
            configure(&mut p, &uri, "clickhouse", &token, version, "String").await;
            let catalog = p
                .request(
                    "oxide/completionSnapshot",
                    params(&uri, &text, text.len(), "clickhouse", &token),
                )
                .await;
            let doc = local(&mut p, &uri, &text, text.len(), "clickhouse").await;
            let supported = operator == "LIKE" || index == 4;
            let escape = if operator == "LIKE" {
                index >= 2
            } else {
                index == 4
            };
            assert_eq!(
                catalog.get("logicalContinuation").and_then(Value::as_bool),
                supported.then_some(true),
                "{operator}: {version:?}"
            );
            assert_eq!(has_escape(&catalog), escape, "{operator}: {version:?}");
            assert_eq!(
                doc.get("logicalContinuation").and_then(Value::as_bool),
                (operator == "LIKE").then_some(true)
            );
            assert!(!has_escape(&doc));
        }
    }
    p.stop().await;
}

#[tokio::test]
async fn matching_continuations_revoke_live_function_type_grants_without_sql_edits() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://pattern-refresh/live.sql";
    let source = "/*😀*/ SELECT 1 WHERE semantic_result() LIKE 'x' ";
    let text = format!("{source}ESuffix;");
    p.open(uri, &text).await;
    for (index, ty) in ["text", "integer", "text"].into_iter().enumerate() {
        let token = format!("pattern-refresh-{index}");
        configure(&mut p, uri, "postgres", &token, None, ty).await;
        let catalog = p
            .request(
                "oxide/completionSnapshot",
                params(uri, &text, source.len() + 1, "postgres", &token),
            )
            .await;
        let doc = local(&mut p, uri, &text, source.len() + 1, "postgres").await;
        assert_eq!(
            catalog.get("logicalContinuation").and_then(Value::as_bool),
            (ty == "text").then_some(true)
        );
        assert_eq!(has_escape(&catalog), ty == "text");
        assert!(doc.get("logicalContinuation").is_none());
        assert!(!has_escape(&doc));
    }
    p.stop().await;
}
