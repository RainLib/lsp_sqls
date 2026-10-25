use super::*;
use super::{
    execute_values::{complete, offset},
    joined_tables::position,
};

async fn configure(p: &mut LspProcess, uri: &str, profile: &str, comment: &str, ty: &str) {
    p.notify("workspace/didChangeConfiguration",json!({"settings":{
        "schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","tables":[
            {"name":"ledger","columns":[{"name":"租金总额","data_type":"decimal","nullable":false},{"name":"amount","data_type":ty,"nullable":false,"comment":comment}]},
            {"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false}]},
            {"name":"archive","columns":[{"name":"secret","data_type":"text","nullable":false,"comment":"Revenue 租金总额"}]}
        ],"functions":[]}],"fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):profile}
    }})).await;
}

#[tokio::test]
async fn native_metadata_search_pinyin_comments_and_projections_accept_real_identifier_edits() {
    let mut p = LspProcess::start().await;
    let mut index = 0;
    for profile in [
        "postgres",
        "mysql",
        "sqlserver",
        "sqlite",
        "hive",
        "clickhouse",
    ] {
        for (marked, name) in [
            ("SELECT l.zj<cursor>suffix FROM ledger l", "租金总额"),
            ("SELECT l.rev<cursor>suffix FROM ledger l", "amount"),
        ] {
            let uri = format!("oxide://metadata-search/{index}.postgres.sql");
            index += 1;
            configure(&mut p, &uri, profile, "营业收入 Revenue amount", "decimal").await;
            let marked = format!("/* 😀 中文 */\r\n{marked};");
            let at = marked.find("<cursor>").unwrap();
            let sql = marked.replace("<cursor>", "");
            p.open(&uri, &sql).await;
            let items = complete(&mut p, &uri, position(&sql, at)).await;
            let item = items
                .iter()
                .find(|i| i["label"] == name && i["kind"] == 5)
                .unwrap_or_else(|| panic!("{profile}: {items:?}"));
            assert_eq!(item["data"]["oxide"]["column"], name);
            assert!(item["detail"].as_str().unwrap().contains("(decimal)"));
            assert!(item["filterText"]
                .as_str()
                .unwrap()
                .contains(if name == "amount" { "rev" } else { "zj" }));
            let edit = &item["textEdit"];
            let from = offset(&sql, &edit["range"]["start"]);
            let to = offset(&sql, &edit["range"]["end"]);
            assert!(sql[from..to].ends_with("suffix"));
            let mut accepted = sql.clone();
            accepted.replace_range(from..to, edit["newText"].as_str().unwrap());
            assert!(accepted.contains(name));
            assert!(!accepted.contains("suffix"));
            assert!(!accepted.contains("Revenue"));
            p.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":accepted.clone()}]})).await;
            p.until(|m| {
                m["method"] == "textDocument/publishDiagnostics"
                    && m["params"]["uri"] == uri
                    && m["params"]["version"] == 2
            })
            .await;
            let hover = p
                .request(
                    "textDocument/hover",
                    json!({"textDocument":{"uri":uri},"position":position(&accepted,from+1)}),
                )
                .await;
            assert!(hover.to_string().contains("decimal"), "{hover}");
            p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
                .await;
        }
    }
    for marked in [
        "WITH q AS(SELECT amount AS net FROM ledger) SELECT q.rev<cursor> FROM q;",
        "WITH q(net) AS(SELECT amount FROM ledger) SELECT q.rev<cursor> FROM q;",
        "SELECT q.rev<cursor> FROM (SELECT amount AS net FROM ledger) q;",
    ] {
        let uri = format!("oxide://metadata-search/{index}.mysql.sql");
        index += 1;
        configure(&mut p, &uri, "mysql", "Revenue", "decimal").await;
        let at = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        p.open(&uri, &sql).await;
        let items = complete(&mut p, &uri, position(&sql, at)).await;
        assert!(
            items.iter().any(|i| i["label"] == "net"
                && i["detail"]
                    .as_str()
                    .is_some_and(|d| d.contains("(decimal)"))),
            "{items:?}"
        );
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}

#[tokio::test]
async fn native_metadata_search_refresh_and_scope_remove_old_candidates_without_sql_changes() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://metadata-search/refresh.mysql.sql";
    let sql = "SELECT l.rev FROM ledger l;";
    configure(&mut p, uri, "mysql", "Revenue", "decimal").await;
    p.open(uri, sql).await;
    let at = sql.find("rev").unwrap() + 3;
    for (comment, ty, present) in [
        ("Revenue", "decimal", true),
        ("other", "bigint", false),
        ("Revenue", "bigint", true),
    ] {
        configure(&mut p, uri, "mysql", comment, ty).await;
        let items = complete(&mut p, uri, position(sql, at)).await;
        let item = items.iter().find(|i| i["label"] == "amount");
        assert_eq!(item.is_some(), present, "{items:?}");
        if let Some(item) = item {
            assert!(item["detail"]
                .as_str()
                .unwrap()
                .contains(&format!("({ty})")));
        }
    }
    p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
        .await;
    for (index, marked) in [
        "SELECT rev<cursor> FROM users;",
        "SELECT 1 FROM ledger; SELECT rev<cursor> FROM users;",
        "SELECT 1 /* rev<cursor> */ FROM ledger;",
        "SELECT 'rev<cursor>' FROM ledger;",
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://metadata-search/negative-{index}.mysql.sql");
        configure(&mut p, &uri, "mysql", "Revenue", "decimal").await;
        let at = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        p.open(&uri, &sql).await;
        let items = complete(&mut p, &uri, position(&sql, at)).await;
        assert!(
            !items
                .iter()
                .any(|i| i["label"] == "amount" || i["label"] == "secret"),
            "{items:?}"
        );
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}
