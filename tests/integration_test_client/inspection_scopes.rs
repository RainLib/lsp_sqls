use super::execute_values::{complete, configure, offset};
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
fn code(items: &[Value], name: &str) -> Vec<Value> {
    items
        .iter()
        .filter(|d| d["code"] == name)
        .cloned()
        .collect()
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
async fn native_inspection_scopes_nested_mutations_keep_completion_and_refresh() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://scopes/mutation.mysql.sql";
    configure(&mut p, uri, "postgres", vec![]).await;
    let sql = "/*😀*/ UPDATE app.users SET id=(SELECT u.i FROM app.users u WHERE u.id=1);";
    p.open(uri, sql).await;
    assert_eq!(code(&diagnostics(&mut p, uri).await, "OXIDE001").len(), 1);
    let at = sql.find("u.i").unwrap() + 3;
    let pos = serde_json::to_value(sql_lsp::position::lsp_position_at_end(&sql[..at])).unwrap();
    let list = complete(&mut p, uri, pos).await;
    let item = list.iter().find(|i| i["label"] == "id").unwrap();
    assert_eq!(item["detail"], "Column: id (int)");
    let edit = &item["textEdit"];
    let mut accepted = sql.to_owned();
    accepted.replace_range(
        offset(sql, &edit["range"]["start"])..offset(sql, &edit["range"]["end"]),
        edit["newText"].as_str().unwrap(),
    );
    assert!(accepted.contains("SELECT u.id FROM"));
    change(&mut p, uri, &accepted, 2).await;
    let result = diagnostics(&mut p, uri).await;
    assert_eq!(code(&result, "OXIDE001").len(), 1);
    assert!(result.iter().all(|d| d["severity"] != 1), "{result:?}");
    let safe = accepted.replace(");", " ) WHERE id=2;");
    change(&mut p, uri, &safe, 3).await;
    assert!(code(&diagnostics(&mut p, uri).await, "OXIDE001").is_empty());
    for (n, (dialect, sql)) in [
        (
            "postgres",
            "WITH q AS(SELECT id FROM users /* noinspection OXIDE001 */) UPDATE users SET id=1;",
        ),
        (
            "postgres",
            "WITH q AS(DELETE FROM users RETURNING id) SELECT id FROM q;",
        ),
        (
            "mysql",
            "UPDATE users SET id=(SELECT id FROM users WHERE id=1);",
        ),
        (
            "sqlserver",
            "CREATE PROC app.p AS BEGIN UPDATE users SET id=1; DELETE FROM users WHERE id=2; END;",
        ),
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://scopes/case-{n}.mysql.sql");
        configure(&mut p, &uri, dialect, vec![]).await;
        p.open(&uri, sql).await;
        let result = diagnostics(&mut p, &uri).await;
        assert_eq!(code(&result, "OXIDE001").len(), 1, "{sql}: {result:?}");
    }
    p.stop().await;
}
#[tokio::test]
async fn native_inspection_scopes_each_join_action_and_quoted_edits_are_local() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://scopes/joins.mysql.sql";
    configure(&mut p, uri, "postgres", vec![]).await;
    let mut sql = "/*😀*/ SELECT a.id FROM a JOIN b JOIN c\nWHERE a.id>0;".to_owned();
    p.open(uri, &sql).await;
    for version in 2..=3 {
        let ds = diagnostics(&mut p, uri).await;
        let joins = code(&ds, "OXIDE002");
        assert_eq!(joins.len(), (4 - version) as usize);
        let actions=p.request("textDocument/codeAction",json!({"textDocument":{"uri":uri},"range":joins[0]["range"],"context":{"diagnostics":[joins[0]],"only":["quickfix"]}})).await;
        let action = actions
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["title"] == "Add non-matching JOIN safety guard")
            .unwrap();
        let edit = &action["edit"]["changes"][uri][0];
        let at = offset(&sql, &edit["range"]["start"]);
        assert!(at < sql.find("WHERE").unwrap());
        sql.insert_str(at, edit["newText"].as_str().unwrap());
        change(&mut p, uri, &sql, version).await;
    }
    let ds = diagnostics(&mut p, uri).await;
    assert!(code(&ds, "OXIDE002").is_empty());
    assert!(ds.iter().all(|d| d["severity"] != 1), "{sql}: {ds:?}");
    for (n, (dialect, rhs)) in [
        ("postgres", "\"Order Table\" AS \"Right Alias\""),
        ("mysql", "`Order Table` AS `Right Alias`"),
        ("sqlserver", "[Order Table] AS [Right Alias]"),
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://scopes/quoted-{n}.mysql.sql");
        configure(&mut p, &uri, dialect, vec![]).await;
        let sql = format!("/*😀*/ SELECT 1 FROM a JOIN {rhs}\nWHERE 1=1;");
        p.open(&uri, &sql).await;
        let ds = code(&diagnostics(&mut p, &uri).await, "OXIDE002");
        assert_eq!(ds.len(), 1);
        let actions=p.request("textDocument/codeAction",json!({"textDocument":{"uri":uri},"range":ds[0]["range"],"context":{"diagnostics":[ds[0]],"only":["quickfix"]}})).await;
        let edit = &actions[0]["edit"]["changes"][&uri][0];
        let at = offset(&sql, &edit["range"]["start"]);
        assert_eq!(at, sql.find('\n').unwrap());
        let mut accepted = sql.clone();
        accepted.insert_str(at, edit["newText"].as_str().unwrap());
        change(&mut p, &uri, &accepted, 2).await;
        let ds = diagnostics(&mut p, &uri).await;
        assert!(code(&ds, "OXIDE002").is_empty());
        assert!(ds.iter().all(|d| d["severity"] != 1), "{accepted}: {ds:?}");
        assert!(accepted.contains(rhs));
    }
    p.stop().await;
}
