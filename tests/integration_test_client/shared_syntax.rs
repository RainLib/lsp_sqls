use super::execute_values::{complete, offset};
use super::*;
async fn diagnostics(p: &mut LspProcess, uri: &str) -> Vec<Value> {
    p.request(
        "textDocument/diagnostic",
        json!({"textDocument":{"uri":uri}}),
    )
    .await["items"]
        .as_array()
        .unwrap()
        .clone()
}
async fn change(p: &mut LspProcess, uri: &str, text: &str, v: i32) {
    p.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":v},"contentChanges":[{"text":text}]}),
    )
    .await;
    p.until(|m| {
        m["method"] == "textDocument/publishDiagnostics"
            && m["params"]["uri"] == uri
            && m["params"]["version"] == v
    })
    .await;
}
fn count(items: &[Value], code: &str) -> usize {
    items.iter().filter(|d| d["code"] == code).count()
}
#[tokio::test]
async fn native_shared_syntax_large_edits_and_two_profiles_preserve_owned_results() {
    let mut p = LspProcess::start().await;
    let large = "oxide://snapshot/large.mysql.sql";
    let small = "oxide://snapshot/small.mysql.sql";
    let id = "aaaaaaaa-1111-4111-8111-111111111111";
    p.notify("workspace/didChangeConfiguration",json!({"settings":{"schemas":[{"id":id,"database":"app","catalog":"db","tables":[{"name":"users","columns":[{"name":"id","data_type":"int","nullable":false}]}],"functions":[]}],"fileSchemas":{(large):id,(small):id},"fileDialects":{(large):"postgres",(small):"sqlserver"}}})).await;
    let sql = format!(
        "/*😀{}*/\r\n{}",
        "padding ".repeat(40000),
        "UPDATE users SET id=1; ".repeat(4097)
    );
    p.open(large, &sql).await;
    let before = diagnostics(&mut p, large).await;
    assert_eq!(count(&before, "OXIDE001"), 4097);
    assert!(before.iter().all(|d| d["severity"] != 1));
    let first = before.iter().find(|d| d["code"] == "OXIDE001").unwrap();
    assert_eq!(first["range"]["start"], json!({"line":0,"character":0}));
    let last = before
        .iter()
        .rev()
        .find(|d| d["code"] == "OXIDE001")
        .unwrap();
    assert_eq!(
        last["range"]["end"],
        serde_json::to_value(sql_lsp::position::lsp_position_at_end(sql.trim_end())).unwrap()
    );
    let marked="/*😀*/ DECLARE @input bigint; WITH q AS(SELECT @input AS value) SELECT q.va<cursor>lue FROM q;";
    let text = marked.replace("<cursor>", "");
    let pos = serde_json::to_value(sql_lsp::position::lsp_position_at_end(
        &marked[..marked.find("<cursor>").unwrap()],
    ))
    .unwrap();
    p.open(small, &text).await;
    let list = complete(&mut p, small, pos.clone()).await;
    let item = list.iter().find(|i| i["label"] == "value").unwrap();
    assert_eq!(item["detail"], "Column: value (bigint)");
    let edit = &item["textEdit"];
    let mut accepted = text.clone();
    accepted.replace_range(
        offset(&text, &edit["range"]["start"])..offset(&text, &edit["range"]["end"]),
        edit["newText"].as_str().unwrap(),
    );
    assert!(accepted.contains("q.value"));
    change(&mut p, small, &accepted, 2).await;
    let after = diagnostics(&mut p, large).await;
    assert_eq!(
        after, before,
        "large original snapshot changed after another provider request"
    );
    change(
        &mut p,
        large,
        "UPDATE users SET id=(SELECT id FROM users WHERE id=1) WHERE id=2;",
        2,
    )
    .await;
    let filtered = diagnostics(&mut p, large).await;
    assert_eq!(count(&filtered, "OXIDE001"), 0);
    assert!(filtered.iter().all(|d| d["severity"] != 1));
    change(
        &mut p,
        large,
        "WITH q AS(DELETE FROM users RETURNING id) SELECT id FROM q; SELECT 1=ANY(+);",
        3,
    )
    .await;
    let invalid = diagnostics(&mut p, large).await;
    assert_eq!(count(&invalid, "OXIDE001"), 1);
    assert!(invalid.iter().any(|d| d["severity"] == 1));
    p.notify(
        "textDocument/didClose",
        json!({"textDocument":{"uri":large}}),
    )
    .await;
    assert!(
        diagnostics(&mut p, large).await.is_empty(),
        "closed document retained an earlier report"
    );
    p.open(large, "DELETE FROM users WHERE id=1;").await;
    let reopened = diagnostics(&mut p, large).await;
    assert_eq!(count(&reopened, "OXIDE001"), 0);
    assert!(reopened.iter().all(|d| d["severity"] != 1));
    let list = complete(&mut p, small, pos).await;
    assert!(list
        .iter()
        .any(|i| i["label"] == "value" && i["detail"] == "Column: value (bigint)"));
    p.stop().await;
}
