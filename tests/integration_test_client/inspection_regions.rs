use super::execute_values::{configure, offset};
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
fn count(items: &[Value], code: &str) -> usize {
    items.iter().filter(|d| d["code"] == code).count()
}
async fn change(p: &mut LspProcess, uri: &str, text: &str, version: i32) {
    p.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":text}]}),
    )
    .await;
    p.until(|m| {
        m["method"] == "textDocument/publishDiagnostics"
            && m["params"]["uri"] == uri
            && m["params"]["version"] == version
    })
    .await;
}
#[tokio::test]
async fn native_inspection_regions_dialects_and_opaque_text_keep_real_diagnostics() {
    let mut p = LspProcess::start().await;
    let cases = [
        (
            "postgres",
            "SELECT $q$; UPDATE users SET id=1; DELETE FROM users;$q$;",
            0,
        ),
        ("mysql", r"SELECT 'it\'s; UPDATE users SET id=1;';", 0),
        (
            "sqlserver",
            "/* outer; /* inner */ UPDATE users SET id=1; */ SELECT 1;",
            0,
        ),
        (
            "clickhouse",
            "// comment; UPDATE users SET id=1;\nSELECT $$; DELETE FROM users;$$;",
            0,
        ),
        ("hive", r"SELECT 'it\'s; DELETE FROM users;';", 0),
        (
            "postgres",
            "UPDATE users SET id=1 WHERE id IN(SELECT 1 WHERE 'a;b'='a;b');",
            0,
        ),
        (
            "mysql",
            "UPDATE users SET id=1 /* noinspection OXIDE001; why */; DELETE FROM users;",
            1,
        ),
        (
            "postgres",
            "UPDATE users SET name='NOINSPECTION OXIDE001';",
            1,
        ),
    ];
    for (n, (dialect, sql, expected)) in cases.iter().enumerate() {
        let uri = format!("oxide://inspection/region-{n}.mysql.sql");
        configure(&mut p, &uri, dialect, vec![]).await;
        p.open(&uri, sql).await;
        let result = diagnostics(&mut p, &uri).await;
        assert_eq!(
            count(&result, "OXIDE001"),
            *expected,
            "{dialect}: {sql}: {result:?}"
        );
    }
    let uri = "oxide://inspection/unfinished.mysql.sql";
    configure(&mut p, uri, "postgres", vec![]).await;
    p.open(
        uri,
        "SELECT 1=ANY(+); SELECT 'unfinished; UPDATE users SET id=1;",
    )
    .await;
    let result = diagnostics(&mut p, uri).await;
    assert_eq!(count(&result, "OXIDE001"), 0);
    assert!(result.iter().any(|d| d["severity"] == 1), "{result:?}");
    p.stop().await;
}
#[tokio::test]
async fn native_inspection_regions_many_warnings_and_document_profile_edits_refresh() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://inspection/refresh.mysql.sql";
    configure(&mut p, uri, "mysql", vec![]).await;
    let sql = "SELECT ilike_count FROM users; UPDATE users SET name='a;b' WHERE id=1;";
    p.open(uri, sql).await;
    let result = diagnostics(&mut p, uri).await;
    assert_eq!(count(&result, "OXIDE001"), 0);
    assert_eq!(count(&result, "OXIDE005"), 0);
    change(
        &mut p,
        uri,
        "UPDATE users SET name='NOINSPECTION OXIDE001';",
        2,
    )
    .await;
    assert_eq!(count(&diagnostics(&mut p, uri).await, "OXIDE001"), 1);
    change(
        &mut p,
        uri,
        "/* noinspection\tOXIDE001 */ UPDATE users SET name='a;b';",
        3,
    )
    .await;
    assert_eq!(count(&diagnostics(&mut p, uri).await, "OXIDE001"), 0);
    let risk = "SELECT name ILIKE 'x', id::integer FROM users;";
    change(&mut p, uri, risk, 4).await;
    for (dialect, expected) in [("mysql", 2), ("postgres", 0), ("clickhouse", 0)] {
        configure(&mut p, uri, dialect, vec![]).await;
        assert_eq!(count(&diagnostics(&mut p, uri).await, "OXIDE005"), expected);
    }
    configure(&mut p, uri, "mysql", vec![]).await;
    let sql = format!("/*😀*/\r\n{}", "UPDATE users SET id=1; ".repeat(4097));
    change(&mut p, uri, &sql, 5).await;
    let result = diagnostics(&mut p, uri).await;
    let warnings = result
        .iter()
        .filter(|d| d["code"] == "OXIDE001")
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 4097);
    assert_eq!(
        warnings[0]["range"]["start"],
        json!({"line":0,"character":0})
    );
    assert_eq!(
        warnings.last().unwrap()["range"]["end"],
        serde_json::to_value(sql_lsp::position::lsp_position_at_end(sql.trim_end())).unwrap()
    );
    p.stop().await;
}
#[tokio::test]
async fn native_inspection_regions_join_action_preserves_literal_and_accepts_utf16_edit() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://inspection/join.mysql.sql";
    let id = "aaaaaaaa-1111-4111-8111-111111111111";
    p.notify("workspace/didChangeConfiguration",json!({"settings":{"schemas":[{"id":id,"database":"app","catalog":"db","functions":[],"tables":[
        {"name":"orders","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"customer_id","data_type":"bigint","nullable":false}],"constraints":[{"name":"orders_customer_fk","constraint_type":"FOREIGN KEY","columns":["customer_id"],"referenced_schema":"app","referenced_table":"customers","referenced_columns":["id"]}]},
        {"name":"customers","columns":[{"name":"id","data_type":"bigint","nullable":false}]}]}],"fileSchemas":{(uri):id},"fileDialects":{(uri):"postgres"}}})).await;
    let sql = "/*😀*/ SELECT 'a;b',o.id FROM app.orders o JOIN app.customers c WHERE c.id>0;";
    p.open(uri, sql).await;
    let result = diagnostics(&mut p, uri).await;
    let warning = result.iter().find(|d| d["code"] == "OXIDE002").unwrap();
    let actions=p.request("textDocument/codeAction",json!({"textDocument":{"uri":uri},"range":warning["range"],"context":{"diagnostics":[warning],"only":["quickfix"]}})).await;
    let action = actions
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Add JOIN condition via orders_customer_fk")
        .unwrap_or_else(|| panic!("{actions:?}"));
    let edit = &action["edit"]["changes"][uri][0];
    let mut accepted = sql.to_owned();
    let start = offset(sql, &edit["range"]["start"]);
    let end = offset(sql, &edit["range"]["end"]);
    assert_eq!(start, sql.find(" WHERE").unwrap());
    assert_eq!(start, end);
    assert_eq!(edit["newText"], " ON o.customer_id = c.id");
    accepted.replace_range(start..end, edit["newText"].as_str().unwrap());
    change(&mut p, uri, &accepted, 2).await;
    let result = diagnostics(&mut p, uri).await;
    assert_eq!(count(&result, "OXIDE002"), 0);
    assert!(result.iter().all(|d| d["severity"] != 1), "{result:?}");
    assert!(accepted.contains("'a;b'"));
    p.stop().await;
}
