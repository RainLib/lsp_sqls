use super::*;

fn configuration(uri: &str, epoch: usize) -> Value {
    let schemas: Vec<_> = (1..=3).map(|source|json!({
        "id":format!("00000000-0000-4000-8000-{source:012}"),"catalog":format!("db{source}"),"database":"app",
        "tables":[{"name":"ledger","columns":[{"name":format!("epoch_{epoch}_source_{source}"),"data_type":"int","nullable":false}]}],"functions":[]
    })).collect();
    json!({"settings":{"schemas":schemas,"fileSchemas":{(uri):"00000000-0000-4000-8000-000000000001"},"fileDialects":{(uri):"postgres"}}})
}

#[tokio::test]
async fn native_burst_metadata_updates_never_mix_three_document_sources() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://analysis/burst.sql";
    let sql="/* 😀 */ INSERT INTO db1.app.ledger VALUES(1); INSERT INTO db2.app.ledger VALUES(2); INSERT INTO db3.app.ledger VALUES(3);";
    p.notify("workspace/didChangeConfiguration", configuration(uri, 0))
        .await;
    p.open(uri, sql).await;
    let mut ids = Vec::new();
    for epoch in 1..=24 {
        p.notify(
            "workspace/didChangeConfiguration",
            configuration(uri, epoch),
        )
        .await;
        p.sequence += 1;
        let id = p.sequence;
        ids.push(id);
        p.send(json!({"jsonrpc":"2.0","id":id,"method":"textDocument/inlayHint","params":{"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":sql.encode_utf16().count()}}}})).await;
    }
    let mut remaining = ids.clone();
    while !remaining.is_empty() {
        let frame = p.frame().await;
        let Some(id) = frame["id"].as_u64() else {
            continue;
        };
        let Some(index) = remaining.iter().position(|expected| *expected == id) else {
            panic!("unexpected response: {frame}")
        };
        remaining.remove(index);
        assert!(frame.get("error").is_none(), "{frame}");
        let hints = frame["result"].as_array().unwrap();
        assert_eq!(hints.len(), 3, "{frame}");
        let labels: Vec<_> = hints.iter().map(|i| i["label"].as_str().unwrap()).collect();
        let epoch = labels[0]
            .strip_prefix("epoch_")
            .unwrap()
            .split('_')
            .next()
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert!(epoch <= 24);
        assert_eq!(
            labels,
            (1..=3)
                .map(|source| format!("epoch_{epoch}_source_{source}:"))
                .collect::<Vec<_>>(),
            "mixed metadata snapshots: {frame}"
        );
    }
    // A final barrier proves the last commit is visible, not just any coherent epoch.
    let final_hints=p.request("textDocument/inlayHint",json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":sql.encode_utf16().count()}}})).await;
    assert_eq!(final_hints[0]["label"], "epoch_24_source_1:");
    p.stop().await;
}

#[tokio::test]
async fn native_incremental_batch_publishes_only_the_final_utf16_version() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://analysis/edits.sql";
    p.notify("workspace/didChangeConfiguration", configuration(uri, 0))
        .await;
    p.open(uri, "/* 😀 */ SELECT 1;").await;
    p.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[
        {"range":{"start":{"line":0,"character":16},"end":{"line":0,"character":17}},"text":"2"},
        {"range":{"start":{"line":0,"character":18},"end":{"line":0,"character":18}},"text":" INSERT INTO db1.app.ledger VALUES(3);"}
    ]})).await;
    p.sequence += 1;
    let id = p.sequence;
    p.send(json!({"jsonrpc":"2.0","id":id,"method":"textDocument/inlayHint","params":{"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1000}}}})).await;
    let mut publications = 0;
    let mut response_received = false;
    loop {
        let frame = p.frame().await;
        if frame["method"] == "textDocument/publishDiagnostics" && frame["params"]["uri"] == uri {
            // Configuration/didOpen may both have queued version 1 before
            // this edit. Earlier versions may precede version 2 on the wire.
            if frame["params"]["version"] == 1 {
                assert_eq!(publications, 0, "old version overtook the new publication");
                continue;
            }
            assert_eq!(frame["params"]["version"], 2);
            publications += 1;
            assert!(
                frame["params"]["diagnostics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|d| d["severity"] != 1),
                "{frame}"
            );
        }
        if frame["id"] == id {
            assert!(frame.get("error").is_none());
            assert_eq!(frame["result"][0]["label"], "epoch_0_source_1:");
            response_received = true;
        }
        // The document can commit before diagnostics reaches the transport.
        // Require both observations without assuming their relative order.
        if response_received && publications > 0 {
            break;
        }
    }
    assert_eq!(
        publications, 1,
        "one analysis publication per content-change batch"
    );
    p.stop().await;
}
