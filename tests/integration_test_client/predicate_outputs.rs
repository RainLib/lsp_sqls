use super::*;

#[tokio::test]
async fn native_predicates_keep_completion_hover_edits_and_diagnostics() {
    let mut process = LspProcess::start().await;
    let id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let cases = [
        (
            "postgres",
            "EXISTS(SELECT * FROM orders o WHERE o.id=u.id)",
            "boolean",
            false,
        ),
        (
            "postgres",
            "NOT EXISTS(SELECT * FROM orders o WHERE o.id=u.id)",
            "boolean",
            false,
        ),
        ("postgres", "u.id IN (1,2,NULL)", "boolean", true),
        (
            "postgres",
            "u.id NOT IN (SELECT id FROM orders)",
            "boolean",
            true,
        ),
        (
            "postgres",
            "u.id IS NOT DISTINCT /* 😀 */ FROM NULL",
            "boolean",
            false,
        ),
        (
            "postgres",
            "u.id>0 AND (u.name ILIKE 'ox%' OR u.id IS NULL)",
            "boolean",
            true,
        ),
        ("postgres", "(SELECT 1) BETWEEN 0 AND 2", "boolean", true),
        (
            "postgres",
            "CAST(NULL AS json)=CAST(NULL AS json)",
            "unknown",
            true,
        ),
        ("postgres", "mystery()=mystery()", "unknown", true),
        (
            "mysql",
            "EXISTS(SELECT * FROM orders o WHERE o.id=u.id)",
            "bigint",
            false,
        ),
        ("mysql", "u.name <=> NULL", "bigint", false),
        (
            "mysql",
            "u.id NOT IN (SELECT id FROM orders)",
            "bigint",
            true,
        ),
        (
            "mysql",
            "CASE WHEN u.id IN (1,2) THEN 1 ELSE 0 END",
            "bigint",
            false,
        ),
        (
            "mysql",
            "CASE WHEN EXISTS(SELECT 1) THEN 1 ELSE NULL END",
            "bigint",
            true,
        ),
        (
            "sqlite",
            "EXISTS(SELECT * FROM orders o WHERE o.id=u.id)",
            "integer",
            false,
        ),
        (
            "sqlite",
            "NOT EXISTS(SELECT * FROM orders o WHERE o.id=u.id)",
            "integer",
            false,
        ),
        ("sqlite", "u.id IN (1,2,NULL)", "integer", true),
        ("sqlite", "u.id IS NULL", "integer", false),
        (
            "sqlserver",
            "CASE WHEN EXISTS(SELECT * FROM orders o WHERE o.id=u.id) THEN 1 ELSE 0 END",
            "int",
            false,
        ),
        (
            "sqlserver",
            "CASE WHEN u.id IN (SELECT id FROM orders) THEN NULL ELSE 1 END",
            "int",
            true,
        ),
    ];
    assert_eq!(cases.len(), 20);
    for (index, (dialect, expression, ty, nullable)) in cases.into_iter().enumerate() {
        let uri = format!("oxide://query/predicates.{index}.mysql.sql");
        process.notify("workspace/didChangeConfiguration", json!({"settings":{
            "schemas":[{"id":id,"database":"app","tables":[
                {"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"name","data_type":"text","nullable":false}]},
                {"name":"orders","columns":[{"name":"id","data_type":"bigint","nullable":false}]}
            ],"functions":[]}],"fileSchemas":{(&uri):id},"fileDialects":{(&uri):dialect}
        }})).await;
        let marked = format!("/* 😀 */\r\nWITH result AS (SELECT {expression} AS value FROM users u) SELECT r.va<cursor>lue FROM result r");
        let offset = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let position = sql_lsp::position::lsp_position_at_end(&sql[..offset]);
        let params = json!({"textDocument":{"uri":uri},"position":position});
        let response = process
            .request("textDocument/completion", params.clone())
            .await;
        let items = response
            .as_array()
            .or_else(|| response["items"].as_array())
            .unwrap();
        let item = items
            .iter()
            .find(|item| item["label"] == "value")
            .unwrap_or_else(|| panic!("{dialect}: {sql}: {response}"));
        assert_eq!(
            item["detail"],
            format!("Column: value ({ty})"),
            "{dialect}: {sql}"
        );
        assert!(
            item["documentation"].to_string().contains(if nullable {
                "Nullable: yes"
            } else {
                "Nullable: no"
            }),
            "{sql}: {item}"
        );
        let edit = &item["textEdit"];
        let start = sql_lsp::position::lsp_position_at_end(&sql[..offset - 2]);
        let end = sql_lsp::position::lsp_position_at_end(&sql[..offset + 3]);
        assert_eq!(edit["range"], json!({"start":start,"end":end}));
        assert_eq!(edit["newText"], "value");
        let hover = process.request("textDocument/hover", params).await;
        assert!(hover.to_string().contains(ty), "{sql}: {hover}");
        assert_eq!(hover["range"]["start"]["line"], 1);
        let diagnostics = process
            .request(
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":uri}}),
            )
            .await;
        assert!(
            diagnostics["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["severity"] != 1 && item["code"] != "PARSE_ERROR"),
            "{dialect}: {sql}: {diagnostics}"
        );
    }
    process.stop().await;
}
