use super::execute_values::{complete, offset};
use super::*;
pub(super) async fn configure(p: &mut LspProcess, uri: &str, dialect: &str, total: &str) {
    p.notify("workspace/didChangeConfiguration",json!({"settings":{
        "schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","tables":[
            {"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"name","data_type":"text","nullable":false}]},
            {"name":"orders","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"total","data_type":total,"nullable":false}]}
        ],"functions":[]}],"fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):dialect}
    }})).await;
}
pub(super) fn position(sql: &str, at: usize) -> Value {
    serde_json::to_value(sql_lsp::position::lsp_position_at_end(&sql[..at])).unwrap()
}
pub(super) async fn diagnostics(p: &mut LspProcess, uri: &str) -> Vec<Value> {
    p.request(
        "textDocument/diagnostic",
        json!({"textDocument":{"uri":uri}}),
    )
    .await["items"]
        .as_array()
        .unwrap()
        .clone()
}
#[tokio::test]
async fn native_joined_tables_profiles_share_real_shapes_and_dialect_validation() {
    let mut p = LspProcess::start().await;
    for (index, dialect) in [
        "postgres",
        "mysql",
        "sqlite",
        "sqlserver",
        "hive",
        "clickhouse",
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://joined-tables/profile-{index}.sql");
        configure(&mut p, &uri, dialect, "decimal").await;
        let marked = "SELECT o.to<cursor>tal FROM (users u JOIN orders o ON u.id=o.id)";
        let at = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        p.open(&uri, &sql).await;
        let items = complete(&mut p, &uri, position(&sql, at)).await;
        assert!(
            items.iter().any(|i| i["label"] == "total"
                && i["detail"].as_str().is_some_and(|d| d.contains("decimal"))),
            "{dialect}: {items:?}"
        );
        let hover = p
            .request(
                "textDocument/hover",
                json!({"textDocument":{"uri":uri},"position":position(&sql,at)}),
            )
            .await;
        assert!(hover.to_string().contains("decimal"), "{dialect}: {hover}");
        let natural_uri = format!("oxide://joined-tables/natural-{index}.sql");
        configure(&mut p, &natural_uri, dialect, "decimal").await;
        p.open(&natural_uri, "SELECT id FROM users NATURAL JOIN orders")
            .await;
        let ds = diagnostics(&mut p, &natural_uri).await;
        assert_eq!(
            ds.iter().any(|d| d["code"] == "UNSUPPORTED_DIALECT_SYNTAX"),
            matches!(*dialect, "sqlserver" | "hive" | "clickhouse"),
            "{dialect}: {ds:?}"
        );
    }
    p.stop().await;
}
#[tokio::test]
async fn native_joined_tables_unfinished_member_acceptance_refresh_and_unicode_edits() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://joined-tables/accepted.sql";
    configure(&mut p, uri, "postgres", "decimal").await;
    let marked = format!(
        "/*{}*/\r\nSELECT g.<cursor> FROM (users u NATURAL LEFT JOIN orders o) g;",
        "😀".repeat(64)
    );
    let at = marked.find("<cursor>").unwrap();
    let sql = marked.replace("<cursor>", "");
    p.open(uri, &sql).await;
    assert!(
        diagnostics(&mut p, uri)
            .await
            .iter()
            .any(|d| d["severity"] == 1),
        "unfinished input still has real syntax diagnostics"
    );
    let items = complete(&mut p, uri, position(&sql, at)).await;
    let item = items
        .iter()
        .find(|i| i["label"] == "total")
        .expect("group member in unfinished SQL");
    let edit = &item["textEdit"];
    let mut accepted = sql.clone();
    accepted.replace_range(
        offset(&sql, &edit["range"]["start"])..offset(&sql, &edit["range"]["end"]),
        edit["newText"].as_str().unwrap(),
    );
    assert!(accepted.contains("SELECT g.total FROM"), "{accepted}");
    p.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":accepted}]}),
    )
    .await;
    p.until(|m| {
        m["method"] == "textDocument/publishDiagnostics"
            && m["params"]["uri"] == uri
            && m["params"]["version"] == 2
    })
    .await;
    assert!(!diagnostics(&mut p, uri)
        .await
        .iter()
        .any(|d| d["severity"] == 1 || d["code"] == "OXIDE004"));
    let at = accepted.find("g.total").unwrap() + "g.to".len();
    for kind in ["numeric", "decimal"] {
        configure(&mut p, uri, "postgres", kind).await;
        let hover = p
            .request(
                "textDocument/hover",
                json!({"textDocument":{"uri":uri},"position":position(&accepted,at)}),
            )
            .await;
        assert!(hover.to_string().contains(kind), "{hover}");
        let items = complete(&mut p, uri, position(&accepted, at)).await;
        assert!(items.iter().any(
            |i| i["label"] == "total" && i["detail"].as_str().is_some_and(|d| d.contains(kind))
        ));
    }
    p.stop().await;
}
