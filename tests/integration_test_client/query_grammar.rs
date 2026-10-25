use super::*;
#[tokio::test]
async fn native_fromless_clauses_and_grouped_sets_keep_types_and_diagnostics() {
    let mut process = LspProcess::start().await;
    let id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let grouped="SELECT ((SELECT u.id WHERE 1=0 UNION ALL SELECT u.id WHERE 1=0) UNION ALL (SELECT u.id WHERE 1=0 UNION ALL SELECT u.id)) AS value FROM users u";
    let cases=[
        ("postgres","SELECT (SELECT u.id WHERE 1=0 UNION ALL SELECT u.id) AS value FROM users u","bigint"),
        ("mysql","SELECT (SELECT u.id WHERE 1=0 UNION ALL SELECT u.id) AS value FROM users u","bigint"),
        ("sqlserver","SELECT (SELECT u.id WHERE 1=0 UNION ALL SELECT u.id) AS value FROM users u","bigint"),
        ("postgres",grouped,"bigint"),("mysql",grouped,"bigint"),("sqlserver",grouped,"bigint"),
        ("postgres","SELECT 1 AS value WHERE 1=1 GROUP BY 1 ORDER BY 1 LIMIT 1","integer"),
        ("postgres","((SELECT id AS value FROM users) UNION ALL (SELECT id AS value FROM orders)) INTERSECT (SELECT id AS value FROM users)","bigint"),
    ];
    assert_eq!(cases.len(), 8);
    for (index, (dialect, body, ty)) in cases.into_iter().enumerate() {
        let uri = format!("oxide://query/query-grammar.{index}.mysql.sql");
        process.notify("workspace/didChangeConfiguration",json!({"settings":{
            "schemas":[{"id":id,"database":"app","tables":[
                {"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false}]},
                {"name":"orders","columns":[{"name":"id","data_type":"bigint","nullable":false}]}
            ],"functions":[]}],"fileSchemas":{(&uri):id},"fileDialects":{(&uri):dialect}
        }})).await;
        let marked =
            format!("/* 😀 */\r\nWITH result AS ({body}) SELECT r.va<cursor>lue FROM result r");
        let offset = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let params = json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..offset])});
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
        assert!(
            item["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains(&format!("({ty})"))),
            "{dialect}: {sql}: {item}"
        );
        let hover = process.request("textDocument/hover", params).await;
        assert!(hover.to_string().contains(ty), "{dialect}: {sql}: {hover}");
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
    // The two original gap probes assert syntax only; scalar cardinality is
    // deliberately outside the language server's static syntax contract.
    for (index, sql) in [
        "SELECT (SELECT 1 WHERE FALSE UNION ALL SELECT 2) AS value",
        "SELECT ((SELECT 1 UNION ALL SELECT 2) UNION ALL (SELECT 3 UNION ALL SELECT 4)) AS value",
        "SELECT 1 WHERE;",
        "SELECT 1 WHERE (+);",
        "SELECT (SELECT 1 UNION ALL) AS value;",
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://query/grammar-probe.{index}.postgres.sql");
        process.open(&uri, sql).await;
        let diagnostics = process
            .request(
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":uri}}),
            )
            .await;
        let invalid = diagnostics["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["severity"] == 1 || item["code"] == "PARSE_ERROR");
        assert_eq!(invalid, index >= 2, "{sql}: {diagnostics}");
    }
    process.stop().await;
}
