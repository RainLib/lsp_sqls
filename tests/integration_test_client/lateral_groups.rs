use super::execute_values::{complete, offset};
use super::joined_tables::{configure, diagnostics, position};
use super::*;
#[tokio::test]
async fn native_lateral_groups_accept_real_outer_members_and_complex_cte_outputs() {
    let mut p = LspProcess::start().await;
    for (index,(dialect,marked)) in [
        ("postgres","SELECT * FROM (users u JOIN orders o USING(id)) g CROSS JOIN LATERAL(SELECT g.to<cursor>tal) d"),
        ("mysql","SELECT * FROM (users u JOIN orders o ON u.id=o.id) CROSS JOIN LATERAL(SELECT o.to<cursor>tal) d"),
        ("sqlserver","SELECT * FROM (users u JOIN orders o ON u.id=o.id) CROSS APPLY(SELECT o.to<cursor>tal) d"),
        ("sqlserver","SELECT * FROM (users u JOIN orders o ON u.id=o.id) OUTER APPLY(SELECT o.to<cursor>tal) d"),
        ("postgres","WITH q AS(SELECT d.* FROM (users u JOIN orders o USING(id)) g CROSS JOIN LATERAL(SELECT g.total AS amount) d) SELECT q.am<cursor>ount FROM q"),
        ("sqlserver","WITH q AS(SELECT d.* FROM (users u JOIN orders o ON u.id=o.id) OUTER APPLY(SELECT o.total AS amount) d) SELECT q.am<cursor>ount FROM q"),
    ].into_iter().enumerate() {
        let uri=format!("oxide://lateral-groups/{index}.sql");configure(&mut p,&uri,dialect,"decimal").await;
        let marked=format!("/*{}*/\r\n{marked};","😀".repeat(64));let at=marked.find("<cursor>").unwrap();let sql=marked.replace("<cursor>","");p.open(&uri,&sql).await;
        let name=if index<4 {"total"}else{"amount"};let items=complete(&mut p,&uri,position(&sql,at)).await;
        let item=items.iter().find(|i|i["label"]==name&&i["detail"].as_str().is_some_and(|d|d.contains("decimal"))).unwrap_or_else(||panic!("{dialect}: {items:?}"));
        let edit=&item["textEdit"];let mut accepted=sql.clone();accepted.replace_range(offset(&sql,&edit["range"]["start"])..offset(&sql,&edit["range"]["end"]),edit["newText"].as_str().unwrap());assert!(accepted.contains(&format!(".{name}")));
        p.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":accepted}]})).await;
        p.until(|m|m["method"]=="textDocument/publishDiagnostics"&&m["params"]["uri"]==uri&&m["params"]["version"]==2).await;
        let hover=p.request("textDocument/hover",json!({"textDocument":{"uri":uri},"position":position(&accepted,at)})).await;assert!(hover.to_string().contains("decimal"),"{hover}");
        assert!(!diagnostics(&mut p,&uri).await.iter().any(|d|d["severity"]==1),"{dialect}: {accepted}");
    }
    p.stop().await;
}
#[tokio::test]
async fn native_lateral_groups_apply_nullability_and_unchanged_sql_refresh() {
    let mut p = LspProcess::start().await;
    for (index, (join, nullable)) in [
        ("CROSS APPLY", "Nullable: no"),
        ("OUTER APPLY", "Nullable: yes"),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://lateral-groups/null-{index}.sql");
        configure(&mut p, &uri, "sqlserver", "decimal").await;
        let sql = format!("SELECT d.value FROM (orders o {join}(SELECT 1 AS value) d)");
        p.open(&uri, &sql).await;
        let at = sql.find("d.value").unwrap() + 4;
        let hover = p
            .request(
                "textDocument/hover",
                json!({"textDocument":{"uri":uri},"position":position(&sql,at)}),
            )
            .await;
        assert!(hover.to_string().contains(nullable), "{join}: {hover}");
    }
    let uri = "oxide://lateral-groups/refreshed.sql";
    let sql =
        "SELECT * FROM (users u JOIN orders o USING(id)) g CROSS JOIN LATERAL(SELECT g.total) d";
    configure(&mut p, uri, "postgres", "decimal").await;
    p.open(uri, sql).await;
    for ty in ["decimal", "numeric", "decimal"] {
        configure(&mut p, uri, "postgres", ty).await;
        let at = sql.find("g.total").unwrap() + 4;
        let items = complete(&mut p, uri, position(sql, at)).await;
        assert!(
            items
                .iter()
                .any(|i| i["label"] == "total"
                    && i["detail"].as_str().is_some_and(|d| d.contains(ty))),
            "{items:?}"
        );
    }
    p.stop().await;
}
