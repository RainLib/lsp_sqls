use super::*;
const URI: &str = "oxide://project/document-only/current.sql";
async fn raw(process: &mut LspProcess, params: Value) -> Value {
    process.sequence += 1;
    let id = process.sequence;
    process
        .send(json!({"jsonrpc":"2.0","id":id,"method":"oxide/documentStructure","params":params}))
        .await;
    process.until(|frame| frame["id"] == id).await
}
fn params(kind: &str, version: i32) -> Value {
    json!({"textDocument":{"uri":URI},"version":version,"dialect":"postgres","kind":kind})
}
#[tokio::test]
async fn native_document_structure_excludes_loaded_physical_metadata() {
    let mut p = LspProcess::start().await;
    p.notify("workspace/didChangeConfiguration",json!({"settings":{"oxideAnalysisToken":"private-context","defaultDialect":"postgres","fileDialects":{(URI):"postgres"},"fileSchemas":{(URI):"00000000-0000-4000-8000-000000000001"},"schemas":[{"id":"00000000-0000-4000-8000-000000000001","database":"app","tables":[{"name":"ledger","columns":[{"name":"id","data_type":"integer","nullable":false}]}],"functions":[{"name":"private_fn","return_type":"integer","parameters":[{"name":"secret_arg","data_type":"integer","optional":false,"mode":"IN"}]}]}]}})).await;
    p.open(URI,"CREATE VIEW local_v AS SELECT 1 AS id;\nWITH q AS (SELECT * FROM ledger) SELECT private_fn(q.id) FROM q;").await;
    let full = p
        .request(
            "textDocument/documentSymbol",
            json!({"textDocument":{"uri":URI}}),
        )
        .await;
    assert!(full
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["name"] == "ledger"));
    assert!(full.to_string().contains("secret_arg"));
    let local = p
        .request("oxide/documentStructure", params("symbols", 1))
        .await;
    assert_eq!(local["version"], 1);
    assert_eq!(local["uri"], URI);
    assert_eq!(local["dialect"], "postgres");
    assert_eq!(local["kind"], "symbols");
    let names = local["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.contains(&"q") && names.contains(&"local_v"));
    assert!(!names.contains(&"ledger") && !names.contains(&"private_fn"));
    assert!(!local.to_string().contains("secret_arg"));
    assert!(local.get("oxideAnalysisToken").is_none());
    p.stop().await;
}
#[tokio::test]
async fn native_document_structure_uses_explicit_profiles_for_complex_unicode_ctes() {
    let mut p = LspProcess::start().await;
    p.open(URI,"/* 漢😀 */\nWITH base AS (\n SELECT 1 AS id\n), q AS (\n SELECT b.* FROM base b\n)\nSELECT q.id FROM q;").await;
    for dialect in [
        "postgres",
        "mysql",
        "sqlite",
        "sqlserver",
        "hive",
        "clickhouse",
    ] {
        for kind in ["symbols", "folding", "selection"] {
            let mut request = params(kind, 1);
            request["dialect"] = json!(dialect);
            if kind == "selection" {
                request["positions"] = json!([{"line":2,"character":10},{"line":6,"character":8}]);
            }
            let result = p.request("oxide/documentStructure", request).await;
            assert_eq!(result["dialect"], dialect);
            assert_eq!(result["kind"], kind);
            assert!(!result["items"].as_array().unwrap().is_empty());
            if kind == "selection" {
                assert_eq!(result["items"].as_array().unwrap().len(), 2);
            }
        }
    }
    p.stop().await;
}
#[tokio::test]
async fn native_document_structure_rejects_old_revisions_and_closed_sources() {
    let mut p = LspProcess::start().await;
    p.open(URI, "WITH q AS (SELECT 1) SELECT * FROM q").await;
    p.notify("textDocument/didChange",json!({"textDocument":{"uri":URI,"version":2},"contentChanges":[{"text":"WITH next_q AS (SELECT 2) SELECT * FROM next_q"}]})).await;
    p.until(|frame| {
        frame["method"] == "textDocument/publishDiagnostics"
            && frame["params"]["uri"] == URI
            && frame["params"]["version"] == 2
    })
    .await;
    assert_eq!(
        raw(&mut p, params("symbols", 1)).await["error"]["code"],
        -32801
    );
    let current = p
        .request("oxide/documentStructure", params("symbols", 2))
        .await;
    assert!(current["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["name"] == "next_q"));
    p.notify("textDocument/didClose", json!({"textDocument":{"uri":URI}}))
        .await;
    // didClose has no diagnostic acknowledgement. Wait for the observable
    // closed-source error, rather than inventing a publishDiagnostics event.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if raw(&mut p, params("symbols", 2)).await["error"]["code"] == -32801 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("closed document must reject structure snapshots");
    p.stop().await;
}
#[tokio::test]
async fn native_document_structure_rejects_metadata_fields_and_invalid_profiles() {
    let mut p = LspProcess::start().await;
    p.open(URI, "SELECT 1").await;
    for key in ["oxideAnalysisToken", "schemas"] {
        let mut request = params("symbols", 1);
        request[key] = json!("forbidden");
        assert_eq!(raw(&mut p, request).await["error"]["code"], -32602);
    }
    for dialect in ["POSTGRES", "missing", "postgres "] {
        let mut request = params("symbols", 1);
        request["dialect"] = json!(dialect);
        assert_eq!(raw(&mut p, request).await["error"]["code"], -32602);
    }
    assert_eq!(
        raw(&mut p, params("workspace", 1)).await["error"]["code"],
        -32602
    );
    assert_eq!(
        raw(&mut p, params("symbols", -1)).await["error"]["code"],
        -32602
    );
    p.stop().await;
}
