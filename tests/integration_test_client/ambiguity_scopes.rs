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
fn columns() -> Value {
    json!([{"name":"orders","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"customer_id","data_type":"bigint","nullable":false}]},{"name":"customers","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"name","data_type":"text","nullable":false}]}])
}
async fn configure(p: &mut LspProcess, uri: &str, dialect: &str, tables: Value) {
    p.notify("workspace/didChangeConfiguration",json!({"settings":{"schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","catalog":"db","tables":tables,"functions":[]}],"fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):dialect}}})).await;
}
fn count(d: &[Value]) -> usize {
    d.iter().filter(|i| i["code"] == "OXIDE004").count()
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
#[tokio::test]
async fn native_ambiguity_scopes_query_shapes_profiles_and_metadata_refresh() {
    let mut p = LspProcess::start().await;
    for (n, (dialect, sql, expected)) in [
        (
            "postgres",
            "SELECT id,(SELECT id FROM customers) AS value FROM orders;",
            0,
        ),
        (
            "postgres",
            "WITH q AS(SELECT id FROM orders) SELECT id FROM q JOIN customers c ON q.id=c.id;",
            1,
        ),
        (
            "postgres",
            "SELECT id FROM orders JOIN customers USING(id);",
            0,
        ),
        (
            "mysql",
            "SELECT id FROM orders a JOIN orders b ON a.id=b.id;",
            1,
        ),
        (
            "sqlserver",
            "SELECT [id] FROM orders a JOIN orders b ON a.id=b.id;",
            1,
        ),
        (
            "clickhouse",
            "SELECT a.id AS id FROM orders a JOIN customers b ON a.id=b.id;",
            0,
        ),
        (
            "hive",
            "SELECT a.id AS id FROM orders a JOIN customers b ON a.id=b.id;",
            0,
        ),
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://ambiguity/case-{n}.mysql.sql");
        configure(&mut p, &uri, dialect, columns()).await;
        p.open(&uri, sql).await;
        let d = diagnostics(&mut p, &uri).await;
        assert_eq!(count(&d), *expected, "{sql}: {d:?}");
    }
    let uri = "oxide://ambiguity/refresh.mysql.sql";
    configure(&mut p, uri, "postgres", columns()).await;
    p.open(
        uri,
        "SELECT id FROM orders o JOIN customers c ON o.customer_id=c.id;",
    )
    .await;
    assert_eq!(count(&diagnostics(&mut p, uri).await), 1);
    let mut updated = columns();
    updated[1]["columns"] = json!([{"name":"name","data_type":"text","nullable":false}]);
    configure(&mut p, uri, "postgres", updated).await;
    assert_eq!(count(&diagnostics(&mut p, uri).await), 0);
    configure(&mut p, uri, "postgres", columns()).await;
    assert_eq!(count(&diagnostics(&mut p, uri).await), 1);
    p.stop().await;
}
#[tokio::test]
async fn native_ambiguity_scopes_quickfix_accepts_local_binding_and_preserves_types() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://ambiguity/fix.mysql.sql";
    configure(&mut p, uri, "postgres", columns()).await;
    let sql =
        "/*😀*/ WITH q AS(SELECT id FROM orders) SELECT id FROM q JOIN customers c ON q.id=c.id;";
    p.open(uri, sql).await;
    let d = diagnostics(&mut p, uri).await;
    let warning = d.iter().find(|d| d["code"] == "OXIDE004").unwrap();
    let actions=p.request("textDocument/codeAction",json!({"textDocument":{"uri":uri},"range":warning["range"],"context":{"diagnostics":[warning],"only":["quickfix"]}})).await;
    let choices: Vec<_> = actions
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["kind"] == "quickfix")
        .collect();
    assert_eq!(choices.len(), 2, "{actions:?}");
    assert!(choices.iter().all(|a| a["isPreferred"] == false));
    let automatic=p.request("textDocument/codeAction",json!({"textDocument":{"uri":uri},"range":{"start":warning["range"]["end"],"end":warning["range"]["end"]},"context":{"diagnostics":[warning]}})).await;
    assert_eq!(
        automatic
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["title"]
                .as_str()
                .is_some_and(|s| s.starts_with("Qualify column as")))
            .count(),
        2,
        "duplicate qualification choices"
    );
    let action = choices
        .iter()
        .find(|a| a["title"] == "Qualify column as q.id")
        .unwrap();
    let edit = &action["edit"]["changes"][uri][0];
    let mut accepted = sql.to_owned();
    accepted.replace_range(
        offset(sql, &edit["range"]["start"])..offset(sql, &edit["range"]["end"]),
        edit["newText"].as_str().unwrap(),
    );
    assert!(accepted.contains("SELECT q.id FROM q"));
    change(&mut p, uri, &accepted, 2).await;
    let d = diagnostics(&mut p, uri).await;
    assert_eq!(count(&d), 0);
    assert!(d.iter().all(|d| d["severity"] != 1), "{d:?}");
    let at = accepted.find("SELECT q.id").unwrap() + "SELECT q.i".len();
    let pos =
        serde_json::to_value(sql_lsp::position::lsp_position_at_end(&accepted[..at])).unwrap();
    let list = complete(&mut p, uri, pos.clone()).await;
    assert!(list
        .iter()
        .any(|i| i["label"] == "id" && i["detail"] == "Column: id (bigint)"));
    let hover = p
        .request(
            "textDocument/hover",
            json!({"textDocument":{"uri":uri},"position":pos}),
        )
        .await;
    assert!(hover.to_string().contains("bigint"));
    let uri2 = "oxide://ambiguity/unicode.mysql.sql";
    configure(&mut p, uri2, "postgres", columns()).await;
    let sql = format!(
        "SELECT (SELECT '{}') AS label, id FROM orders o JOIN customers c ON o.customer_id=c.id;",
        "😀".repeat(64)
    );
    p.open(uri2, &sql).await;
    let d = diagnostics(&mut p, uri2).await;
    let warning = d.iter().find(|d| d["code"] == "OXIDE004").unwrap();
    let actions=p.request("textDocument/codeAction",json!({"textDocument":{"uri":uri2},"range":warning["range"],"context":{"diagnostics":[warning],"only":["quickfix"]}})).await;
    let action = actions
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Qualify column as o.id")
        .unwrap();
    let edit = &action["edit"]["changes"][uri2][0];
    let mut accepted = sql.clone();
    accepted.replace_range(
        offset(&sql, &edit["range"]["start"])..offset(&sql, &edit["range"]["end"]),
        edit["newText"].as_str().unwrap(),
    );
    assert!(accepted.contains(", o.id FROM"));
    change(&mut p, uri2, &accepted, 2).await;
    let d = diagnostics(&mut p, uri2).await;
    assert_eq!(count(&d), 0);
    assert!(d.iter().all(|d| d["severity"] != 1));
    p.stop().await;
}
