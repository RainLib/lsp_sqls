use super::*;

#[tokio::test]
async fn configuration_receipt_requires_the_current_committed_token_without_documents() {
    let mut process = LspProcess::start().await;
    let params = |token: &str| json!({"oxideAnalysisToken":token});
    assert_eq!(
        process
            .request("oxide/analysisConfiguration", params("first"))
            .await,
        json!({"applied":false})
    );
    for token in ["first", "second"] {
        process.notify("workspace/didChangeConfiguration", json!({"settings":{"oxideAnalysisToken":token,"defaultDialect":"postgres","schemas":[]}})).await;
        assert_eq!(
            process
                .request("oxide/analysisConfiguration", params(token))
                .await,
            json!({"applied":true,"oxideAnalysisToken":token})
        );
    }
    assert_eq!(
        process
            .request("oxide/analysisConfiguration", params("first"))
            .await,
        json!({"applied":false})
    );
    process.stop().await;
}

#[tokio::test]
async fn configuration_receipt_rejects_invalid_params_at_the_wire_boundary() {
    let mut process = LspProcess::start().await;
    for params in [
        json!({}),
        json!({"oxideAnalysisToken":null}),
        json!({"oxideAnalysisToken":3}),
        json!({"oxideAnalysisToken":"a b"}),
        json!({"oxideAnalysisToken":"x".repeat(129)}),
    ] {
        process.sequence += 1;
        let id = process.sequence;
        process.send(json!({"jsonrpc":"2.0","id":id,"method":"oxide/analysisConfiguration","params":params})).await;
        let response = process.until(|message| message["id"] == id).await;
        assert_eq!(response["error"]["code"], -32602);
    }
    process.stop().await;
}

#[tokio::test]
async fn configuration_receipt_is_revoked_by_legacy_configuration() {
    let mut process = LspProcess::start().await;
    process
        .notify(
            "workspace/didChangeConfiguration",
            json!({"settings":{"oxideAnalysisToken":"old"}}),
        )
        .await;
    assert_eq!(
        process
            .request(
                "oxide/analysisConfiguration",
                json!({"oxideAnalysisToken":"old"})
            )
            .await["applied"],
        true
    );
    process
        .notify(
            "workspace/didChangeConfiguration",
            json!({"settings":{"defaultDialect":"mysql"}}),
        )
        .await;
    assert_eq!(
        process
            .request(
                "oxide/analysisConfiguration",
                json!({"oxideAnalysisToken":"old"})
            )
            .await,
        json!({"applied":false})
    );
    process.stop().await;
}

fn catalog(token: &str, column: &str) -> Value {
    json!({"oxideAnalysisToken":token,"defaultDialect":"postgres","schemas":[{"id":"00000000-0000-4000-8000-000000000001","database":"app","tables":[{"name":"ledger","columns":[{"name":column,"data_type":"bigint","nullable":false}]}],"functions":[]}],"fileSchemas":{"oxide://config/current.sql":"00000000-0000-4000-8000-000000000001"},"fileDialects":{"oxide://config/current.sql":"postgres"}})
}
async fn config_receipt(p: &mut LspProcess, token: &str) -> Value {
    p.request(
        "oxide/analysisConfiguration",
        json!({"oxideAnalysisToken":token}),
    )
    .await
}
#[tokio::test]
async fn malformed_settings_revoke_both_receipts_without_partially_replacing_a_warm_catalog() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://config/current.sql";
    p.open(uri, "SELECT value_old FROM ledger;").await;
    let mut missing_functions = catalog("bad", "value_new");
    missing_functions["schemas"][0]
        .as_object_mut()
        .unwrap()
        .remove("functions");
    for bad in [
        missing_functions,
        json!(null),
        json!([]),
        json!({"oxideAnalysisToken":"bad","schemas":null}),
        json!({"oxideAnalysisToken":"bad","defaultDialect":"missing"}),
        json!({"oxideAnalysisToken":"bad","fileSchemas":{(uri):"invalid"}}),
        json!({"oxideAnalysisToken":"bad","fileDialects":{(uri):"missing"}}),
        json!({"oxideAnalysisToken":"bad","completionPreferences":{"keywordCase":"auto"}}),
    ] {
        p.notify(
            "workspace/didChangeConfiguration",
            json!({"settings":catalog("old","value_old")}),
        )
        .await;
        assert_eq!(config_receipt(&mut p, "old").await["applied"], true);
        p.notify("workspace/didChangeConfiguration", json!({"settings":bad}))
            .await;
        assert_eq!(
            config_receipt(&mut p, "bad").await,
            json!({"applied":false})
        );
        assert_eq!(
            config_receipt(&mut p, "old").await,
            json!({"applied":false})
        );
        let retained = p
            .request(
                "textDocument/documentSymbol",
                json!({"textDocument":{"uri":uri}}),
            )
            .await;
        assert!(
            retained
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["name"] == "ledger"),
            "last valid catalog retained: {retained}"
        );
        for token in ["old", "bad"] {
            p.sequence += 1;
            let id = p.sequence;
            p.send(json!({"jsonrpc":"2.0","id":id,"method":"oxide/documentDiagnostics","params":{"textDocument":{"uri":uri},"version":1,"dialect":"postgres","mode":"metadata","oxideAnalysisToken":token}})).await;
            let rejected = p.until(|frame| frame["id"] == id).await;
            assert_eq!(rejected["error"]["code"], -32801);
        }
    }
    p.stop().await;
}
#[tokio::test]
async fn a_valid_repair_replaces_same_text_cte_fields_after_rejection_and_empty_catalogs_are_valid()
{
    let mut p = LspProcess::start().await;
    let uri = "oxide://config/current.sql";
    p.notify(
        "workspace/didChangeConfiguration",
        json!({"settings":catalog("old","value_old")}),
    )
    .await;
    let text = "WITH q AS (SELECT l.* FROM ledger l) SELECT q.vaSuffix FROM q;";
    p.open(uri, text).await;
    let at = text.find("vaSuffix").unwrap() + 2;
    let params = json!({"textDocument":{"uri":uri},"position":{"line":0,"character":at}});
    let before = p.request("textDocument/completion", params.clone()).await;
    assert!(before.to_string().contains("value_old"));
    p.notify(
        "workspace/didChangeConfiguration",
        json!({"settings":{"oxideAnalysisToken":"bad","schemas":null}}),
    )
    .await;
    assert_eq!(config_receipt(&mut p, "bad").await["applied"], false);
    p.notify(
        "workspace/didChangeConfiguration",
        json!({"settings":catalog("repaired","value_new")}),
    )
    .await;
    assert_eq!(config_receipt(&mut p, "repaired").await["applied"], true);
    let repaired = p.request("textDocument/completion", params).await;
    assert!(
        repaired.to_string().contains("value_new") && !repaired.to_string().contains("value_old")
    );
    p.notify("workspace/didChangeConfiguration",json!({"settings":{"oxideAnalysisToken":"clear","schemas":[],"fileSchemas":{},"fileDialects":{}}})).await;
    assert_eq!(config_receipt(&mut p, "clear").await["applied"], true);
    assert_eq!(config_receipt(&mut p, "repaired").await["applied"], false);
    p.stop().await;
}
