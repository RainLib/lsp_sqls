use super::*;
use super::{execute_values::complete, joined_tables::position};

async fn configure(p: &mut LspProcess, uri: &str, remote: &str) {
    let local = "aaaaaaaa-1111-4111-8111-111111111111";
    let other = "bbbbbbbb-2222-4222-8222-222222222222";
    p.notify("workspace/didChangeConfiguration", json!({"settings":{
        "schemas":[
            {"id":local,"catalog":"db","database":"app","tables":[{"name":"ledger","columns":[{"name":"amount","data_type":"decimal","nullable":false,"comment":"Revenue local"}]}],"functions":[]},
            {"id":other,"server":remote,"catalog":"db","database":"app","tables":[{"name":"ledger","columns":[{"name":"amount","data_type":"nvarchar","nullable":false,"comment":"Revenue remote"}]}],"functions":[]}
        ],"fileSchemas":{(uri):local},"fileDialects":{(uri):"sqlserver"}
    }})).await;
}

#[tokio::test]
async fn namespace_bound_direct_cte_and_join_projections_keep_their_own_types() {
    let mut p = LspProcess::start().await;
    for (index, (marked, ty)) in [
        ("SELECT r.rev<cursor>suffix FROM remote.db.app.ledger r", "nvarchar"),
        ("SELECT r.rev<cursor>suffix FROM db.app.ledger r", "decimal"),
        ("WITH q AS (SELECT amount FROM remote.db.app.ledger) SELECT q.amount<cursor>suffix FROM q", "nvarchar"),
        ("SELECT q.amount<cursor>suffix FROM (SELECT amount FROM remote.db.app.ledger) q", "nvarchar"),
        ("WITH q AS (SELECT r.amount FROM db.app.ledger l JOIN remote.db.app.ledger r ON 1=1) SELECT q.amount<cursor>suffix FROM q", "nvarchar"),
        ("WITH q AS (SELECT l.amount FROM db.app.ledger l JOIN remote.db.app.ledger r ON 1=1) SELECT q.amount<cursor>suffix FROM q", "decimal"),
    ].into_iter().enumerate() {
        let uri = format!("oxide://namespace/{index}.sql");
        configure(&mut p, &uri, "remote").await;
        let at = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        p.open(&uri, &sql).await;
        let items = complete(&mut p, &uri, position(&sql, at)).await;
        let item = items.iter().find(|i| i["label"] == "amount").unwrap_or_else(||panic!("{marked}: {items:?}"));
        assert!(item["detail"].as_str().unwrap().contains(ty), "{marked}: {item}");
        let edit = &item["textEdit"];
        let from = super::execute_values::offset(&sql, &edit["range"]["start"]);
        let to = super::execute_values::offset(&sql, &edit["range"]["end"]);
        assert!(sql[from..to].ends_with("suffix"));
        assert_eq!(edit["newText"], "amount");
        let resolved = p.request("completionItem/resolve", item.clone()).await;
        assert!(resolved.to_string().contains(ty), "{resolved}");
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}})).await;
    }
    p.stop().await;
}

#[tokio::test]
async fn unknown_servers_and_server_only_refresh_cannot_reuse_local_metadata() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://namespace/refresh.sql";
    let sql = "SELECT r.rev FROM remote.db.app.ledger r";
    configure(&mut p, uri, "other").await;
    p.open(uri, sql).await;
    let at = sql.find("rev").unwrap() + 3;
    for (server, expected) in [("other", false), ("remote", true), ("other", false)] {
        configure(&mut p, uri, server).await;
        let items = complete(&mut p, uri, position(sql, at)).await;
        assert_eq!(
            items.iter().any(|i| i["label"] == "amount"),
            expected,
            "{server}: {items:?}"
        );
        if expected {
            let item = items.iter().find(|i| i["label"] == "amount").unwrap();
            assert_eq!(item["data"]["oxide"]["server"], "remote");
            assert!(item["detail"].as_str().unwrap().contains("nvarchar"));
        }
    }
    p.stop().await;
}
