use super::*;
#[tokio::test]
async fn native_reviewed_alias_snapshots_preserve_wire_identity_and_local_bindings() {
    let mut p = LspProcess::start().await;
    let text = "WITH q AS (SELECT CAST(1 AS INTEGER) AS local_count) SELECT q.local_count FROM q;";
    p.open(URI, text).await;
    for dialect in [
        "duckdb",
        "mariadb",
        "postgresql",
        "mssql",
        "argo",
        "turso",
        "tidb",
        "cockroachdb",
    ] {
        let mut request = params(json!({"kind":"hover","position":point(text,"q.local_count")}));
        request["dialect"] = json!(dialect);
        let semantic = p.request("oxide/documentSemantics", request).await;
        assert_eq!(semantic["dialect"], dialect);
        assert!(!semantic["value"].is_null(), "{dialect}");
        let structure = p
            .request(
                "oxide/documentStructure",
                json!({"textDocument":{"uri":URI},"version":1,"dialect":dialect,"kind":"symbols"}),
            )
            .await;
        assert_eq!(structure["dialect"], dialect);
        assert!(structure["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == "q"));
        let diagnostic = p
            .request(
                "oxide/documentDiagnostics",
                json!({"textDocument":{"uri":URI},"version":1,"dialect":dialect,"mode":"syntax"}),
            )
            .await;
        assert_eq!(diagnostic["dialect"], dialect);
        assert!(diagnostic["diagnostics"].is_array());
        assert!(diagnostic.get("oxideAnalysisToken").is_none());
    }
    p.stop().await;
}
#[tokio::test]
async fn native_diagnostic_alias_authority_distinguishes_registered_adapters() {
    let mut p = LspProcess::start().await;
    p.open(URI, "SELECT 1;").await;
    for configured in ["sqlite", "duckdb"] {
        let token = format!("alias-{configured}");
        p.notify("workspace/didChangeConfiguration", json!({"settings":{"oxideAnalysisToken":token,"defaultDialect":configured,"fileDialects":{(URI):configured}}})).await;
        assert_eq!(
            p.request(
                "oxide/analysisConfiguration",
                json!({"oxideAnalysisToken":token})
            )
            .await["applied"],
            true
        );
        p.sequence += 1;
        let id = p.sequence;
        p.send(json!({"jsonrpc":"2.0","id":id,"method":"oxide/documentDiagnostics","params":{"textDocument":{"uri":URI},"version":1,"dialect":"duckdb","mode":"metadata","oxideAnalysisToken":token}})).await;
        let response = p.until(|frame| frame["id"] == id).await;
        if configured == "sqlite" {
            assert_eq!(response["error"]["code"], -32801);
        } else {
            assert_eq!(response["result"]["dialect"], "duckdb");
            assert_eq!(response["result"]["oxideAnalysisToken"], token);
        }
    }
    p.stop().await;
}
const URI: &str = "oxide://project/local-semantic/current.sql";
fn params(operation: Value) -> Value {
    json!({"textDocument":{"uri":URI},"version":1,"dialect":"postgres","operation":operation})
}
fn point(text: &str, needle: &str) -> Value {
    let offset = text.rfind(needle).unwrap() + needle.len() - usize::from(!needle.ends_with('('));
    let prefix = &text[..offset];
    json!({"line":prefix.bytes().filter(|b|*b==b'\n').count(),"character":prefix.rsplit('\n').next().unwrap().encode_utf16().count()})
}
async fn raw(p: &mut LspProcess, params: Value) -> Value {
    p.sequence += 1;
    let id = p.sequence;
    p.send(json!({"jsonrpc":"2.0","id":id,"method":"oxide/documentSemantics","params":params}))
        .await;
    p.until(|frame| frame["id"] == id).await
}
#[tokio::test]
async fn native_local_semantics_preserve_capabilities_without_reading_warm_physical_shapes() {
    let mut p = LspProcess::start().await;
    p.notify("workspace/didChangeConfiguration",json!({"settings":{"oxideAnalysisToken":"private-context","defaultDialect":"postgres","fileDialects":{(URI):"postgres"},"fileSchemas":{(URI):"00000000-0000-4000-8000-000000000001"},"schemas":[{"id":"00000000-0000-4000-8000-000000000001","database":"app","tables":[{"name":"ledger","columns":[{"name":"secret_value","data_type":"integer","nullable":false}]}],"functions":[{"name":"private_fn","return_type":"integer","parameters":[{"name":"secret_arg","data_type":"integer","optional":false,"mode":"IN"}]}]}]}})).await;
    let text="/* 漢😀 */\nWITH q AS (SELECT 1 AS local_id) SELECT COALESCE(q.local_id, 2) FROM q;\nSELECT private_fn(1), secret_value FROM ledger;\nINSERT INTO ledger(explicit_value) VALUES(2);";
    p.open(URI, text).await;
    let full = p
        .request(
            "textDocument/signatureHelp",
            json!({"textDocument":{"uri":URI},"position":point(text,"private_fn(")}),
        )
        .await;
    assert!(full.to_string().contains("secret_arg"));
    let range = json!({"start":{"line":0,"character":0},"end":{"line":3,"character":45}});
    for operation in [
        json!({"kind":"hover","position":point(text,"q.local_id")}),
        json!({"kind":"definitions","position":point(text,"q.local_id")}),
        json!({"kind":"references","position":point(text,"FROM q"),"includeDeclaration":true}),
        json!({"kind":"signature","position":point(text,"COALESCE(")}),
        json!({"kind":"prepareRename","position":point(text,"FROM q")}),
        json!({"kind":"rename","position":point(text,"FROM q"),"newName":"new_q"}),
        json!({"kind":"codeActions","range":range,"context":{"diagnostics":[],"only":["source"]}}),
        json!({"kind":"tokens"}),
        json!({"kind":"inlayHints","range":range}),
    ] {
        let kind = operation["kind"].as_str().unwrap().to_owned();
        let result = p
            .request("oxide/documentSemantics", params(operation))
            .await;
        assert_eq!(result["kind"], kind);
        assert_eq!(result["uri"], URI);
        assert_eq!(result["version"], 1);
        assert!(!result["value"].is_null(), "{kind}: {result}");
        assert!(!result["value"].to_string().contains("secret_arg"));
        assert!(!result["value"].to_string().contains("secret_value:"));
        assert!(result.get("oxideAnalysisToken").is_none());
    }
    let local = p
        .request(
            "oxide/documentSemantics",
            params(json!({"kind":"signature","position":point(text,"private_fn(")})),
        )
        .await;
    assert!(local["value"].is_null());
    let full = p
        .request(
            "textDocument/signatureHelp",
            json!({"textDocument":{"uri":URI},"position":point(text,"private_fn(")}),
        )
        .await;
    assert!(full.to_string().contains("secret_arg"));
    p.stop().await;
}
#[tokio::test]
async fn native_local_semantics_cover_six_explicit_profiles_and_cte_star_actions() {
    let mut p = LspProcess::start().await;
    let text="/* 漢😀 */\nWITH base AS (SELECT 1 AS local_id), q AS (SELECT b.* FROM base b) SELECT q.local_id FROM q;";
    p.open(URI, text).await;
    for dialect in [
        "postgres",
        "mysql",
        "sqlite",
        "sqlserver",
        "hive",
        "clickhouse",
    ] {
        for operation in [
            json!({"kind":"hover","position":point(text,"q.local_id")}),
            json!({"kind":"definitions","position":point(text,"q.local_id")}),
            json!({"kind":"references","position":point(text,"FROM q"),"includeDeclaration":true}),
        ] {
            let mut request = params(operation);
            request["dialect"] = json!(dialect);
            let result = p.request("oxide/documentSemantics", request).await;
            assert_eq!(result["dialect"], dialect);
            assert!(!result["value"].is_null());
            if let Some(items) = result["value"].as_array() {
                assert!(!items.is_empty(), "{dialect}: {result}");
                assert!(items.iter().all(|item| item["uri"] == URI));
            }
        }
    }
    p.stop().await;
}
#[tokio::test]
async fn native_cte_rename_keeps_shadowed_bindings_and_other_documents_untouched() {
    let mut p = LspProcess::start().await;
    let text="WITH q AS (SELECT 1 AS id) SELECT q.id FROM q WHERE EXISTS (WITH q AS (SELECT 2 AS id) SELECT q.id FROM q); SELECT q.id FROM ledger q;";
    p.open(URI, text).await;
    p.open(
        "oxide://project/local-semantic/other.sql",
        "WITH q AS (SELECT 2) SELECT * FROM q;",
    )
    .await;
    let result = p
        .request(
            "oxide/documentSemantics",
            params(json!({"kind":"rename","position":{"line":0,"character":44},"newName":"new_q"})),
        )
        .await;
    let changes = result["value"]["changes"].as_object().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[URI].as_array().unwrap().len(), 3);
    p.stop().await;
}
#[tokio::test]
async fn native_local_semantics_reject_stale_and_closed_sources() {
    let mut p = LspProcess::start().await;
    p.open(URI, "SELECT 1").await;
    let mut old = params(json!({"kind":"tokens"}));
    old["version"] = json!(0);
    assert_eq!(raw(&mut p, old).await["error"]["code"], -32801);
    p.notify("textDocument/didClose", json!({"textDocument":{"uri":URI}}))
        .await;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let result = raw(&mut p, params(json!({"kind":"tokens"}))).await;
        if result["error"]["code"] == -32801 {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    p.stop().await;
}
#[tokio::test]
async fn native_local_semantic_contract_rejects_authority_fields_and_unknown_operations() {
    let mut p = LspProcess::start().await;
    p.open(URI, "SELECT 1").await;
    for operation in [
        json!({"kind":"tokens","schema":"private"}),
        json!({"kind":"hover"}),
        json!({"kind":"signature","position":{"line":-1,"character":0}}),
        json!({"kind":"execute"}),
    ] {
        assert_eq!(
            raw(&mut p, params(operation)).await["error"]["code"],
            -32602
        );
    }
    let mut request = params(json!({"kind":"tokens"}));
    request["oxideAnalysisToken"] = json!("private");
    assert_eq!(raw(&mut p, request).await["error"]["code"], -32602);
    p.stop().await;
}
