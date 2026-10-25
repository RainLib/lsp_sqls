use super::*;
use super::{
    execute_values::{complete, offset},
    joined_tables::position,
};

async fn configure(
    p: &mut LspProcess,
    uri: &str,
    default: Option<&str>,
    primary: bool,
    complete: bool,
) {
    let id = "aaaaaaaa-1111-4111-8111-111111111111";
    let context = default.map(|s| json!({"default_schema":s,"tables_complete":complete}));
    let table = |ty: &str, comment: &str| json!({"name":"ledger","columns":[{"name":"amount","data_type":ty,"nullable":false,"comment":comment}]});
    p.notify("workspace/didChangeConfiguration",json!({"settings":{
        "schemas":[
            {"id":id,"catalog":"db","database":"dbo","namespace":context,"tables":[table("decimal","Revenue dbo")],"functions":[]},
            {"id":"bbbbbbbb-2222-4222-8222-222222222222","catalog":"db","database":"sales","namespace":context,"tables":if primary{vec![table("uuid","Revenue sales")]}else{vec![]},"functions":[]}
        ],"fileSchemas":{(uri):id},"fileDialects":{(uri):"sqlserver"}
    }})).await;
}

#[tokio::test]
async fn effective_default_schema_and_proven_dbo_fallback_keep_native_edits_and_types() {
    let mut p = LspProcess::start().await;
    for (i, (marked, primary, ty)) in [
        ("SELECT r.rev<cursor>suffix FROM db..ledger r", true, "uuid"),
        (
            "SELECT r.rev<cursor>suffix FROM [db]..[ledger] r",
            false,
            "decimal",
        ),
        (
            "WITH q AS (SELECT amount AS net FROM db..ledger) SELECT q.net<cursor>suffix FROM q",
            true,
            "uuid",
        ),
        (
            "SELECT q.net<cursor>suffix FROM (SELECT amount AS net FROM db..ledger) q",
            false,
            "decimal",
        ),
        (
            "SELECT r.rev<cursor>suffix FROM db.dbo.ledger r",
            true,
            "decimal",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://default-schema/{i}.sql");
        configure(&mut p, &uri, Some("sales"), primary, true).await;
        let at = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        p.open(&uri, &sql).await;
        let items = complete(&mut p, &uri, position(&sql, at)).await;
        let item = items
            .iter()
            .find(|i| i["label"] == "amount" || i["label"] == "net")
            .unwrap_or_else(|| panic!("{marked}: {items:?}"));
        assert!(
            item["detail"].as_str().unwrap().contains(ty),
            "{marked}: {item}"
        );
        let edit = &item["textEdit"];
        let from = offset(&sql, &edit["range"]["start"]);
        let to = offset(&sql, &edit["range"]["end"]);
        assert!(sql[from..to].ends_with("suffix"));
        let accepted = format!(
            "{}{}{}",
            &sql[..from],
            edit["newText"].as_str().unwrap(),
            &sql[to..]
        );
        assert!(!accepted.contains("suffix"));
        let resolved = p.request("completionItem/resolve", item.clone()).await;
        assert!(resolved.to_string().contains(ty));
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
        let hover = p
            .request(
                "textDocument/hover",
                json!({"textDocument":{"uri":uri},"position":position(&accepted,from+1)}),
            )
            .await;
        assert!(hover.to_string().contains(ty), "{marked}: {hover}");
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}

#[tokio::test]
async fn missing_context_partial_coverage_and_same_sql_refresh_never_assume_dbo() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://default-schema/refresh.sql";
    let sql = "SELECT r.rev FROM db..ledger r";
    let at = sql.find("rev").unwrap() + 3;
    configure(&mut p, uri, None, false, true).await;
    p.open(uri, sql).await;
    for (default, primary, coverage, ty) in [
        (None, false, true, None),
        (Some("sales"), false, false, None),
        (Some("sales"), false, true, Some("decimal")),
        (Some("sales"), true, false, Some("uuid")),
        (Some("dbo"), true, false, Some("decimal")),
        (None, true, true, None),
    ] {
        configure(&mut p, uri, default, primary, coverage).await;
        let items = complete(&mut p, uri, position(sql, at)).await;
        if let Some(ty) = ty {
            assert!(
                items.iter().any(|i| i["label"] == "amount"
                    && i["detail"].as_str().is_some_and(|d| d.contains(ty))),
                "{items:?}"
            );
        } else {
            assert!(!items.iter().any(|i| i["kind"] == 5), "{items:?}");
        }
    }
    p.stop().await;
}
