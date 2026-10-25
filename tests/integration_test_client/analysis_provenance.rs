use super::*;

#[tokio::test]
async fn native_diagnostic_provenance_follows_configuration_without_sql_edits() {
    let mut process = LspProcess::start().await;
    let uri = "oxide://provenance/current.sql";
    let sql = "WITH q AS (SELECT a.* FROM app.a a) SELECT shared FROM q x JOIN app.b b ON 1=1";
    for (token, ambiguous) in [("first-context", true), ("second-context", false)] {
        process.notify("workspace/didChangeConfiguration",json!({"settings":{
            "oxideAnalysisToken":token,"fileDialects":{(uri):"postgres"},
            "fileSchemas":{(uri):"00000000-0000-4000-8000-000000000001"},
            "schemas":[{"id":"00000000-0000-4000-8000-000000000001","database":"app","tables":[
                {"name":"a","columns":[{"name":"shared","data_type":"int","nullable":false}]},
                {"name":"b","columns":[{"name":if ambiguous {"shared"}else{"different"},"data_type":"int","nullable":false}]}],"functions":[]}]}})).await;
        if token == "first-context" {
            process.notify("textDocument/didOpen",json!({"textDocument":{"uri":uri,"languageId":"postgres","version":1,"text":sql}})).await;
        }
        let publication = process
            .until(|frame| {
                frame["method"] == "textDocument/publishDiagnostics"
                    && frame["params"]["uri"] == uri
                    && frame["params"]["oxideAnalysisToken"] == token
            })
            .await;
        assert_eq!(publication["params"]["version"], 1);
        let collision = publication["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "OXIDE004");
        assert_eq!(
            collision, ambiguous,
            "diagnostics and token must come from the same configuration"
        );
    }
}

#[tokio::test]
async fn native_missing_token_configuration_does_not_retain_a_previous_grant() {
    let mut process = LspProcess::start().await;
    let uri = "oxide://provenance/clear.sql";
    process
        .notify(
            "workspace/didChangeConfiguration",
            json!({"settings":{"oxideAnalysisToken":"valid-token"}}),
        )
        .await;
    process.notify("textDocument/didOpen",json!({"textDocument":{"uri":uri,"languageId":"postgres","version":1,"text":"SELECT 1"}})).await;
    process
        .until(|frame| {
            frame["method"] == "textDocument/publishDiagnostics"
                && frame["params"]["oxideAnalysisToken"] == "valid-token"
        })
        .await;
    process
        .notify(
            "workspace/didChangeConfiguration",
            json!({"settings":{"defaultDialect":"postgres"}}),
        )
        .await;
    let publication = process
        .until(|frame| {
            frame["method"] == "textDocument/publishDiagnostics" && frame["params"]["uri"] == uri
        })
        .await;
    assert!(publication["params"].get("oxideAnalysisToken").is_none());
}

#[tokio::test]
async fn native_snapshot_recovers_missed_diagnostics_and_keeps_schema_free_mode() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://snapshot/recovery.sql";
    p.notify("workspace/didChangeConfiguration",json!({"settings":{"oxideAnalysisToken":"snapshot-current","fileDialects":{(uri):"postgres"},"fileSchemas":{(uri):"00000000-0000-4000-8000-000000000001"},"schemas":[{"id":"00000000-0000-4000-8000-000000000001","database":"app","tables":[{"name":"a","columns":[{"name":"shared","data_type":"int","nullable":false}]},{"name":"b","columns":[{"name":"shared","data_type":"int","nullable":false}]}],"functions":[]}]}})).await;
    p.open(
        uri,
        "WITH q AS (SELECT a.* FROM app.a a) SELECT shared FROM q x JOIN app.b b ON 1=1",
    )
    .await;
    let request = json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","mode":"metadata","oxideAnalysisToken":"snapshot-current"});
    let result = p
        .request("oxide/documentDiagnostics", request.clone())
        .await;
    assert_eq!(result["oxideAnalysisToken"], "snapshot-current");
    assert!(result["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"] == "OXIDE004"));
    let mut syntax = request;
    syntax["mode"] = json!("syntax");
    syntax.as_object_mut().unwrap().remove("oxideAnalysisToken");
    let result = p.request("oxide/documentDiagnostics", syntax).await;
    assert!(result.get("oxideAnalysisToken").is_none());
    assert!(!result["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"] == "OXIDE004"));
    p.stop().await;
}

#[tokio::test]
async fn native_snapshot_uses_explicit_dialect_and_utf16_syntax_ranges() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://snapshot/syntax.sql";
    p.open(uri, "SELECT '漢😀';\nSELECT (1 + );").await;
    for dialect in [
        "postgres",
        "mysql",
        "sqlserver",
        "sqlite",
        "hive",
        "clickhouse",
    ] {
        let result = p
            .request(
                "oxide/documentDiagnostics",
                json!({"textDocument":{"uri":uri},"version":1,"dialect":dialect,"mode":"syntax"}),
            )
            .await;
        assert_eq!(result["dialect"], dialect);
        assert_eq!(result["version"], 1);
        assert!(result["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "SYNTAX_ERROR" && d["range"]["start"]["line"] == 1));
    }
    p.stop().await;
}

#[tokio::test]
async fn native_snapshot_rejects_stale_versions_and_tokens() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://snapshot/stale.sql";
    p.notify(
        "workspace/didChangeConfiguration",
        json!({"settings":{"oxideAnalysisToken":"current","fileDialects":{(uri):"postgres"}}}),
    )
    .await;
    p.open(uri, "SELECT 1").await;
    for (version, token) in [(2, "current"), (1, "old")] {
        p.sequence += 1;
        let id = p.sequence;
        p.send(json!({"jsonrpc":"2.0","id":id,"method":"oxide/documentDiagnostics","params":{"textDocument":{"uri":uri},"version":version,"dialect":"postgres","mode":"metadata","oxideAnalysisToken":token}})).await;
        let result = p.until(|frame| frame["id"] == id).await;
        assert_eq!(result["error"]["code"], -32801);
    }
    p.stop().await;
}
