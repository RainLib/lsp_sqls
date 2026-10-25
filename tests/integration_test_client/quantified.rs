use super::*;

#[tokio::test]
async fn native_array_and_quantified_query_field_edits_keep_delimiters() {
    let mut process = LspProcess::start().await;
    let uris: Vec<_> = (0..6)
        .map(|index| format!("oxide://query/array-cursor.{index}.mysql.sql"))
        .collect();
    let id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let file_schemas: serde_json::Map<String, Value> =
        uris.iter().map(|uri| (uri.clone(), json!(id))).collect();
    let file_dialects: serde_json::Map<String, Value> = uris
        .iter()
        .map(|uri| (uri.clone(), json!("postgres")))
        .collect();
    process.notify("workspace/didChangeConfiguration", json!({"settings":{
        "schemas":[{"id":id,"database":"app","tables":[
            {"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"name","data_type":"text","nullable":true},{"name":"email","data_type":"text","nullable":true}]},
            {"name":"orders","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"status","data_type":"text","nullable":true}]}
        ],"functions":[]}],"fileSchemas":file_schemas,"fileDialects":file_dialects
    }})).await;
    let cases = [
        ("SELECT u.name ILIKE ANY(ARRAY[COALESCE(u.na<cursor>me,'')]) FROM users u", "name"),
        ("SELECT ARRAY[na<cursor>me] FROM users", "name"),
        ("SELECT ARRAY [COALESCE(na<cursor>me,'')] FROM users", "name"),
        ("SELECT u.name ~ ANY(ARRAY[COALESCE(u.em<cursor>ail,'')]) FROM users u", "email"),
        ("SELECT u.id FROM users u WHERE u.id=ANY(SELECT o.id FROM orders o WHERE o.sta<cursor>tus=u.email)", "status"),
        ("SELECT u.id FROM users u WHERE u.id=ANY(SELECT o.id FROM orders o WHERE o.status=u.em<cursor>ail)", "email"),
    ];
    assert_eq!(cases.len(), 6);
    for (index, (marked, name)) in cases.into_iter().enumerate() {
        let uri = &uris[index];
        let marked = format!("/* 😀 */\r\n{marked}");
        let offset = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(uri, &sql).await;
        let params = json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..offset])});
        let response = process
            .request("textDocument/completion", params.clone())
            .await;
        let item = response
            .as_array()
            .or_else(|| response["items"].as_array())
            .unwrap()
            .iter()
            .find(|item| item["label"] == name)
            .unwrap_or_else(|| panic!("{sql}: {response}"));
        assert!(item["detail"].as_str().unwrap().contains("text"), "{item}");
        let start = offset - if name == "status" { 3 } else { 2 };
        let end = start + name.len();
        assert_eq!(
            item["textEdit"]["range"],
            json!({"start":sql_lsp::position::lsp_position_at_end(&sql[..start]),"end":sql_lsp::position::lsp_position_at_end(&sql[..end])}),
            "{sql}: {item}"
        );
        assert_eq!(item["textEdit"]["newText"], name);
        let mut accepted = sql.clone();
        accepted.replace_range(start..end, item["textEdit"]["newText"].as_str().unwrap());
        assert_eq!(
            accepted, sql,
            "accepting an unchanged field preserves all SQL delimiters"
        );
        let hover = process.request("textDocument/hover", params).await;
        assert!(
            hover.to_string().contains("Column") && hover.to_string().contains("text"),
            "{sql}: {hover}"
        );
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
            "{sql}: {diagnostics}"
        );
    }
    process.stop().await;
}

#[tokio::test]
async fn native_quantified_search_and_row_outputs_preserve_semantics_and_edits() {
    let mut process = LspProcess::start().await;
    let id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let cases = [
        ("postgres", "u.id=ANY(SELECT o.id FROM orders o WHERE o.id=u.id)", "boolean", true),
        ("postgres", "u.id>SOME(SELECT id FROM orders)", "boolean", true),
        ("postgres", "u.id<=ALL(SELECT id FROM orders WHERE 1=0)", "boolean", true),
        ("postgres", "u.id=ANY(ARRAY[1,2,NULL])", "boolean", true),
        ("postgres", "u.id=ANY(u.ids)", "boolean", true),
        ("postgres", "u.name ILIKE ANY(ARRAY['ox%','db%'])", "boolean", true),
        ("postgres", "u.name ~ ALL(ARRAY['^ox','de$'])", "boolean", true),
        ("postgres", "to_tsvector('english',u.name) @@ ANY(ARRAY[to_tsquery('english','oxide'),to_tsquery('english','dbx')])", "boolean", true),
        ("postgres", "NULL=ANY(ARRAY[]::integer[])", "boolean", false),
        ("postgres", "NULL<>ALL(CAST(ARRAY[] AS integer[]))", "boolean", false),
        ("postgres", "1=ANY(NULL::integer[])", "boolean", true),
        ("postgres", "(u.id,u.name)=(1,'oxide')", "boolean", true),
        ("postgres", "ROW(u.id,u.name)>=ROW(1,'oxide')", "boolean", true),
        ("postgres", "(u.id,u.name)=(SELECT o.id,o.status FROM orders o WHERE o.id=u.id)", "boolean", true),
        ("postgres", "(u.id,u.name)=ANY(SELECT id,status FROM orders)", "boolean", true),
        ("postgres", "(u.id,2) BETWEEN (0,0) AND (100,3)", "boolean", true),
        ("postgres", "(1,2)=(1,2,3)", "unknown", true),
        ("postgres", "ROW(1,CAST(NULL AS custom_type))=ROW(1,CAST(NULL AS custom_type))", "unknown", true),
        ("mysql", "u.id=ANY(SELECT id FROM orders)", "bigint", true),
        ("mysql", "u.id>SOME(SELECT id FROM orders)", "bigint", true),
        ("mysql", "u.id<>ALL(SELECT id FROM orders)", "bigint", true),
        ("mysql", "ROW(u.id,u.name)=ROW(1,'oxide')", "bigint", true),
        ("mysql", "(u.id,u.name)=(SELECT o.id,o.status FROM orders o WHERE o.id=u.id)", "bigint", true),
        ("mysql", "CASE WHEN u.id=ANY(SELECT id FROM orders) THEN 1 ELSE 0 END", "bigint", false),
        ("sqlite", "(u.id,u.name)=(1,'oxide')", "integer", true),
        ("sqlite", "(u.id,2) BETWEEN (0,0) AND (100,3)", "integer", true),
        ("sqlite", "(u.id,u.name)=(SELECT o.id,o.status FROM orders o WHERE o.id=u.id)", "integer", true),
        ("sqlserver", "CASE WHEN u.id>ALL(SELECT id FROM orders) THEN 1 ELSE 0 END", "int", false),
        ("sqlserver", "CASE WHEN u.id=ANY(SELECT id FROM orders) THEN NULL ELSE 1 END", "int", true),
    ];
    assert_eq!(cases.len(), 29);
    for (index, (dialect, expression, ty, nullable)) in cases.into_iter().enumerate() {
        let uri = format!("oxide://query/quantified.{index}.mysql.sql");
        process.notify("workspace/didChangeConfiguration", json!({"settings":{
            "schemas":[{"id":id,"database":"app","tables":[
                {"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"name","data_type":"text","nullable":false},{"name":"ids","data_type":"integer[]","nullable":true}]},
                {"name":"orders","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"status","data_type":"text","nullable":true}]}
            ],"functions":[]}],"fileSchemas":{(&uri):id},"fileDialects":{(&uri):dialect}
        }})).await;
        let marked = format!("/* 😀 */\r\nWITH result AS (SELECT {expression} AS value FROM users u) SELECT r.va<cursor>lue FROM result r");
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
        assert_eq!(
            item["textEdit"]["range"],
            json!({"start":sql_lsp::position::lsp_position_at_end(&sql[..offset-2]),"end":sql_lsp::position::lsp_position_at_end(&sql[..offset+3])})
        );
        assert_eq!(item["textEdit"]["newText"], "value");
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
    for (index, sql) in [
        "SELECT 1=ANY(SELECT);",
        "SELECT (1,2)=(,);",
        "SELECT 1=ANY(+);",
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://query/quantified-error.{index}.postgres.sql");
        process.open(&uri, sql).await;
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
                .any(|item| item["severity"] == 1 || item["code"] == "PARSE_ERROR"),
            "{sql}: {diagnostics}"
        );
    }
    process.stop().await;
}
