use super::*;

#[tokio::test]
async fn unicode_where_columns_keep_the_process_live_and_offer_typed_values() {
    let mut p = LspProcess::start().await;
    for (index, profile) in [
        "postgres",
        "mysql",
        "sqlserver",
        "sqlite",
        "clickhouse",
        "hive",
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://where/unicode-{index}.mysql.sql");
        p.notify("workspace/didChangeConfiguration", json!({"settings": {
            "defaultDialect": profile,
            "schemas": [{"id":"00000000-0000-4000-8000-000000000080","database":"app","functions":[],"tables":[{"name":"jobs","columns":[{"name":"用户名称","data_type":"text","nullable":true}]}]}],
            "fileSchemas":{(uri.as_str()):"00000000-0000-4000-8000-000000000080"},"fileDialects":{(uri.as_str()):profile}
        }})).await;
        let reference = match *profile {
            "mysql" | "hive" => "`用户名称`",
            "sqlserver" => "[用户名称]",
            _ => "\"用户名称\"",
        };
        let sql = format!("/* 😀 */\nSELECT * FROM jobs WHERE {reference}");
        p.open(&uri, &sql).await;
        for end in sql
            .char_indices()
            .map(|(at, _)| at)
            .filter(|at| *at > sql.find("WHERE").unwrap())
        {
            let _ = p.request("textDocument/completion", json!({"textDocument":{"uri":uri},"position":super::joined_tables::position(&sql,end)})).await;
        }
        let sql = format!("{sql} = ");
        p.notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":sql}]}),
        )
        .await;
        let items = super::execute_values::complete(
            &mut p,
            &uri,
            super::joined_tables::position(&sql, sql.len()),
        )
        .await;
        assert!(
            items.iter().any(|item| item["label"] == "NULL"),
            "{profile}: {items:?}"
        );
        assert!(
            items.iter().any(|item| item["label"] == "''"),
            "{profile}: {items:?}"
        );
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}

#[tokio::test]
async fn where_buffer_inherits_ambiguous_catalog_route_and_closes_without_closing_main() {
    let mut p = LspProcess::start().await;
    let source = "oxide://query/table.mysql.sql";
    let uri = "oxide://where/input.mysql.sql?source=oxide%3A%2F%2Fquery%2Ftable.mysql.sql";
    let first = "00000000-0000-4000-8000-000000000001";
    let second = "00000000-0000-4000-8000-000000000002";
    let schema = |id: &str, catalog: &str, name: &str| json!({"id":id,"catalog":catalog,"database":"app","functions":[],"tables":[{"name":"users","columns":[{"name":name,"data_type":"int","nullable":false}]}]});
    p.notify("workspace/didChangeConfiguration",json!({"settings":{"oxideAnalysisToken":"where-route","defaultDialect":"mysql","schemas":[schema(first,"db1","value_first"),schema(second,"db2","value_second")],"fileSchemas":{source:second},"fileDialects":{source:"sqlserver"}}})).await;
    let main = "DECLARE @local TABLE([UNIQUE] int); SELECT q.UN FROM @local q;";
    p.open(source, main).await;
    let condition = "SELECT * FROM app.users WHERE valSuffix = 1;";
    p.open(uri, condition).await;
    let at = condition.find("valSuffix").unwrap() + 3;
    let list = p
        .request(
            "textDocument/completion",
            json!({"textDocument":{"uri":uri},"position":{"line":0,"character":at}}),
        )
        .await;
    assert!(list.to_string().contains("value_second"), "{list}");
    assert!(!list.to_string().contains("value_first"), "{list}");
    p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
        .await;
    let at = main.find("q.UN").unwrap() + 4;
    let main_list = p
        .request(
            "textDocument/completion",
            json!({"textDocument":{"uri":source},"position":{"line":0,"character":at}}),
        )
        .await;
    assert!(main_list.to_string().contains("UNIQUE"), "{main_list}");
    p.stop().await;
}
