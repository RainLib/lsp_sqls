use super::execute_values::{complete, offset};
use super::*;
fn tables(kind: &str) -> Value {
    json!([
        {"name":"orders","columns":[{"name":"id","data_type":kind,"nullable":false},{"name":"total","data_type":"decimal","nullable":false}]},
        {"name":"users","columns":[{"name":"id","data_type":"int","nullable":false},{"name":"name","data_type":"text","nullable":false}]},
        {"name":"audit_events","columns":[{"name":"audit_secret","data_type":"text","nullable":false}]}
    ])
}
async fn configure(p: &mut LspProcess, uri: &str, dialect: &str, kind: &str) {
    p.notify("workspace/didChangeConfiguration",json!({"settings":{"schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","catalog":"db","tables":tables(kind),"functions":[]}],"fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):dialect}}})).await;
}
fn position(sql: &str, at: usize) -> Value {
    serde_json::to_value(sql_lsp::position::lsp_position_at_end(&sql[..at])).unwrap()
}
async fn diagnostic_items(p: &mut LspProcess, uri: &str) -> Vec<Value> {
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
async fn native_join_operands_profiles_keep_completion_and_aliases_in_current_inputs() {
    let mut p = LspProcess::start().await;
    for (n, dialect) in [
        "postgres",
        "mysql",
        "sqlite",
        "hive",
        "clickhouse",
        "sqlserver",
    ]
    .iter()
    .enumerate()
    {
        for (m,marked) in [
            "SELECT o.id FROM orders o JOIN users u ON audit_<cursor>='x' JOIN audit_events later ON true;",
            "SELECT o.id FROM audit_events a, orders o JOIN users u ON audit_<cursor>='x';",
            "SELECT o.id FROM orders o JOIN users u ON la<cursor>=1 JOIN audit_events later ON true;",
            "SELECT o.id FROM orders o JOIN users u ON tot<cursor>=1 JOIN orders later ON later.id=u.id;",
            "SELECT o.id FROM orders o LEFT JOIN users u ON EXISTS(SELECT o.tot<cursor>) JOIN orders later ON true;",
            "SELECT o.id FROM orders o LEFT JOIN users u ON EXISTS(SELECT later.tot<cursor>) JOIN orders later ON true;",
        ].iter().enumerate() {
            let uri=format!("oxide://join-operands/{n}-{m}.mysql.sql");
            configure(&mut p,&uri,dialect,"bigint").await;
            let at=marked.find("<cursor>").unwrap();let sql=marked.replace("<cursor>","");
            p.open(&uri,&sql).await;
            let items=complete(&mut p,&uri,position(&sql,at)).await;
            if *dialect=="sqlite" && m<3 {
                let expected=if m==2 {"later"} else {"audit_secret"};
                assert!(items.iter().any(|i|i["label"]==expected),"{dialect}: {items:?}");
            } else {
                assert!(!items.iter().any(|i|i["label"]=="later" || i["label"].as_str().is_some_and(|s|s.contains("audit_secret"))),"{dialect}: {items:?}");
            }
            if m==3 {
                let fields:Vec<_>=items.iter().filter(|i|i["kind"]==5).collect();
                if *dialect=="sqlite" {
                    assert_eq!(fields.len(),2,"{items:?}");
                    assert!(fields.iter().any(|i|i["label"]=="o.total")&&fields.iter().any(|i|i["label"]=="later.total"));
                } else { assert_eq!(fields.len(),1,"{dialect}: {items:?}"); assert_eq!(fields[0]["label"],"total"); }
                assert!(fields[0]["detail"].as_str().unwrap().contains("decimal"));
                let ds=diagnostic_items(&mut p,&uri).await;assert!(!ds.iter().any(|d|d["code"]=="OXIDE004" || d["severity"]==1),"{dialect}: {ds:?}");
            }
            if m==4 {
                let fields:Vec<_>=items.iter().filter(|i|i["kind"]==5).collect();
                assert_eq!(fields.len(),2,"{dialect}: {fields:?}");
                assert!(fields.iter().any(|i|i["label"]=="total" && i["detail"]=="Column: total (decimal)"));
                assert!(fields.iter().all(|i|i["label"]=="id" || i["label"]=="total"));
            }
            if m==5 {assert!(!items.iter().any(|i|i["kind"]==5),"{dialect}: {items:?}");}
        }
    }
    p.stop().await;
}
#[tokio::test]
async fn native_join_operands_unicode_quickfix_accepts_only_current_rows_and_refreshes_types() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://join-operands/accepted.mysql.sql";
    configure(&mut p, uri, "postgres", "bigint").await;
    let sql=format!("/*{}*/\r\nSELECT p.id FROM orders JOIN orders p ON id=p.id JOIN orders later ON later.id=p.id;","😀".repeat(64));
    p.open(uri, &sql).await;
    let ds = diagnostic_items(&mut p, uri).await;
    let warnings: Vec<_> = ds.iter().filter(|d| d["code"] == "OXIDE004").collect();
    assert_eq!(warnings.len(), 1, "{ds:?}");
    let warning = warnings[0];
    let actions=p.request("textDocument/codeAction",json!({"textDocument":{"uri":uri},"range":warning["range"],"context":{"diagnostics":[warning],"only":["quickfix"]}})).await;
    let choices: Vec<_> = actions
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| {
            a["title"]
                .as_str()
                .is_some_and(|t| t.starts_with("Qualify column as "))
        })
        .collect();
    assert_eq!(choices.len(), 2, "{choices:?}");
    assert!(choices.iter().all(|a| a["isPreferred"] == false));
    assert!(choices
        .iter()
        .all(|a| !a["title"].as_str().unwrap().contains("later")));
    let action = choices
        .iter()
        .find(|a| a["title"] == "Qualify column as orders.id")
        .unwrap();
    let edit = &action["edit"]["changes"][uri][0];
    let mut accepted = sql.clone();
    accepted.replace_range(
        offset(&sql, &edit["range"]["start"])..offset(&sql, &edit["range"]["end"]),
        edit["newText"].as_str().unwrap(),
    );
    assert!(accepted.contains("ON orders.id=p.id JOIN orders later"));
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
    let ds = diagnostic_items(&mut p, uri).await;
    assert!(
        !ds.iter()
            .any(|d| d["code"] == "OXIDE004" || d["severity"] == 1),
        "{ds:?}"
    );
    let at = accepted.find("ON orders.id").unwrap() + "ON orders.i".len();
    for kind in ["int", "bigint"] {
        configure(&mut p, uri, "postgres", kind).await;
        let hover = p
            .request(
                "textDocument/hover",
                json!({"textDocument":{"uri":uri},"position":position(&accepted,at)}),
            )
            .await;
        assert!(
            serde_json::to_string(&hover)
                .unwrap()
                .contains(&format!("Type: {kind}\\n")),
            "{hover:?}"
        );
        let items = complete(&mut p, uri, position(&accepted, at)).await;
        assert!(
            items
                .iter()
                .any(|i| i["label"] == "id" && i["detail"] == format!("Column: id ({kind})")),
            "{items:?}"
        );
    }
    p.stop().await;
}
