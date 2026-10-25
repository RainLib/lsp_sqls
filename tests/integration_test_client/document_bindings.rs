use super::joined_tables::position;
use super::*;

async fn configure(p: &mut LspProcess, uri: &str, profile: &str, column: &str) -> String {
    static NEXT_CONFIGURATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let token = format!(
        "document-bindings-{profile}-{column}-{}",
        NEXT_CONFIGURATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let table = |name: &str, column: &str| json!({"name":name,"columns":[{"name":column,"data_type":"int","nullable":false}]});
    p.notify("workspace/didChangeConfiguration",json!({"settings":{
        "oxideAnalysisToken":token,"schemas":[
            {"id":"aaaaaaaa-1111-4111-8111-111111111111","catalog":"db","database":"app","tables":[table("ledger","local_value"),table("collision","shared")],"functions":[]},
            {"id":"bbbbbbbb-2222-4222-8222-222222222222","catalog":"other","database":"app","tables":[table("ledger",column),table("collision",if column=="other_value"{"shared"}else{"distinct"})],"functions":[]},
            {"id":"cccccccc-3333-4333-8333-333333333333","catalog":"db","database":"sales","tables":[table("ledger","sales_value"),table("collision","shared")],"functions":[]}
        ],"fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):profile}
    }})).await;
    token
}
async fn hints(p: &mut LspProcess, uri: &str, sql: &str) -> Value {
    p.request("textDocument/inlayHint",json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":position(sql,sql.len())}})).await
}
fn labels(items: &Value) -> Vec<&str> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect()
}
fn ambiguity(items: &Value) -> usize {
    items
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["code"] == "OXIDE004")
        .count()
}

#[tokio::test]
async fn native_document_hints_and_pushed_pulled_inline_ambiguity_use_bound_sources() {
    let mut p = LspProcess::start().await;
    for profile in [
        "postgres",
        "mysql",
        "sqlite",
        "hive",
        "clickhouse",
        "sqlserver",
    ] {
        let uri = format!("oxide://document-bindings/{profile}.sql");
        configure(&mut p, &uri, profile, "other_value").await;
        let sql="/* 😀 */\r\nINSERT INTO db.app.ledger VALUES(1);\r\nINSERT INTO other.app.ledger VALUES(2); INSERT INTO sales.ledger SELECT 3;\r\nSELECT shared FROM db.app.collision a JOIN other.app.collision b ON 1=1;";
        p.open(&uri, sql).await;
        assert_eq!(
            labels(&hints(&mut p, &uri, sql).await),
            ["local_value:", "other_value:", "sales_value:"],
            "{profile}"
        );
        let pulled = p
            .request(
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":uri}}),
            )
            .await;
        assert_eq!(ambiguity(&pulled["items"]), 1, "{profile}: {pulled}");
        let validated = p
            .request(
                "oxide/validateInlineCompletion",
                json!({"textDocument":{"uri":uri},"text":sql}),
            )
            .await;
        assert_eq!(ambiguity(&validated), 1, "{profile}: {validated}");
        let qualified = sql.replace("SELECT shared FROM", "SELECT a.shared FROM");
        p.notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":qualified}]}),
        )
        .await;
        let pushed = p
            .until(|m| {
                m["method"] == "textDocument/publishDiagnostics"
                    && m["params"]["uri"] == uri
                    && m["params"]["version"] == 2
            })
            .await;
        assert_eq!(ambiguity(&pushed["params"]["diagnostics"]), 0);
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}

#[tokio::test]
async fn native_schema_refresh_updates_same_document_hints_and_ambiguity_without_stale_bindings() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://document-bindings/refresh.sql";
    let sql="INSERT INTO other.app.ledger VALUES(1); SELECT shared FROM db.app.collision a JOIN other.app.collision b ON 1=1;";
    let initial_token = configure(&mut p, uri, "sqlserver", "other_value").await;
    p.open(uri, sql).await;
    // open() already consumes its notification. Establish the initial state
    // through a pull rather than waiting for the same notification twice.
    let initial = p
        .request(
            "textDocument/diagnostic",
            json!({"textDocument":{"uri":uri}}),
        )
        .await;
    assert_eq!(ambiguity(&initial["items"]), 1);
    assert_eq!(
        p.request(
            "oxide/analysisConfiguration",
            json!({"oxideAnalysisToken":initial_token})
        )
        .await["applied"],
        true
    );
    for (column, count) in [("updated", 0), ("other_value", 1), ("updated", 0)] {
        let token = configure(&mut p, uri, "sqlserver", column).await;
        let pushed = p
            .until(|m| {
                m["method"] == "textDocument/publishDiagnostics"
                    && m["params"]["uri"] == uri
                    && m["params"]["version"] == 1
                    && m["params"]["oxideAnalysisToken"] == token
            })
            .await;
        assert_eq!(ambiguity(&pushed["params"]["diagnostics"]), count);
        // This process adapter discards unmatched notifications while waiting
        // for a response. Confirm the commit after consuming its notification.
        assert_eq!(
            p.request(
                "oxide/analysisConfiguration",
                json!({"oxideAnalysisToken":token})
            )
            .await["applied"],
            true
        );
        assert_eq!(
            labels(&hints(&mut p, uri, sql).await),
            [format!("{column}:").as_str()]
        );
        let pulled = p
            .request(
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":uri}}),
            )
            .await;
        assert_eq!(ambiguity(&pulled["items"]), count);
    }
    p.stop().await;
}
