use super::*;

fn parameter(name: &str, ty: &str, optional: bool, mode: Option<&str>) -> Value {
    json!({"name":name,"data_type":ty,"optional":optional,"mode":mode})
}
fn routine(kind: &str, parameters: Vec<Value>) -> Value {
    json!({"name":"calc","routine_type":kind,"return_type":"integer","parameters":parameters})
}
fn ordinary(second: &str, ty: &str, optional: bool) -> Value {
    routine(
        "function",
        vec![
            parameter("first", "integer", optional, None),
            parameter(second, ty, optional, None),
        ],
    )
}
async fn configure(process: &mut LspProcess, uri: &str, dialect: &str, functions: Vec<Value>) {
    process.notify("workspace/didChangeConfiguration",json!({"settings":{
        "schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","catalog":"db","tables":[{"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false,"is_primary_key":true}]}],"functions":functions}],
        "fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):dialect}
    }})).await;
}
async fn named_items(process: &mut LspProcess, uri: &str, position: Value) -> Vec<Value> {
    let result = process
        .request(
            "textDocument/completion",
            json!({"textDocument":{"uri":uri},"position":position}),
        )
        .await;
    result
        .as_array()
        .or_else(|| result["items"].as_array())
        .unwrap()
        .iter()
        .filter(|item| {
            item["detail"]
                .as_str()
                .is_some_and(|detail| detail.starts_with("Parameter:"))
        })
        .cloned()
        .collect()
}
fn byte_offset(text: &str, position: &Value) -> usize {
    let mut line = 0;
    let mut column = 0;
    for (offset, ch) in text.char_indices() {
        if line == position["line"].as_u64().unwrap()
            && column == position["character"].as_u64().unwrap()
        {
            return offset;
        }
        if ch == '\n' {
            line += 1;
            column = 0;
        } else {
            column += ch.len_utf16() as u64;
        }
    }
    assert_eq!(
        (line, column),
        (
            position["line"].as_u64().unwrap(),
            position["character"].as_u64().unwrap()
        )
    );
    text.len()
}
async fn semantic_check(
    process: &mut LspProcess,
    uri: &str,
    sql: &str,
    position: Value,
    label: &str,
    expected_hints: &[&str],
) {
    let help = process
        .request(
            "textDocument/signatureHelp",
            json!({"textDocument":{"uri":uri},"position":position}),
        )
        .await;
    assert_eq!(help["signatures"][0]["label"], label, "{sql}: {help}");
    assert_eq!(help["activeParameter"], 1, "{sql}: {help}");
    let hints=process.request("textDocument/inlayHint",json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":sql_lsp::position::lsp_position_at_end(sql)}})).await;
    assert_eq!(
        hints
            .as_array()
            .unwrap()
            .iter()
            .map(|hint| hint["label"].as_str().unwrap())
            .collect::<Vec<_>>(),
        expected_hints,
        "{sql}: {hints}"
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
            .all(|item| item["severity"] != 1),
        "{sql}: {diagnostics}"
    );
}

#[tokio::test]
async fn native_named_arguments_bind_reordered_complex_values_defaults_modes_and_quotes() {
    let mut process = LspProcess::start().await;
    let standard = ordinary("second", "integer", false);
    let cases=[
        ("postgres","SELECT calc(second=><cursor>1,first=>2)",vec![standard.clone()],"calc(first integer, second integer)",vec![]),
        ("postgresql","SELECT calc(second:=<cursor>1,first:=2)",vec![standard.clone()],"calc(first integer, second integer)",vec![]),
        ("postgres","SELECT calc(1,second=><cursor>2)",vec![standard.clone()],"calc(first integer, second integer)",vec!["first:"]),
        ("postgres","SELECT calc(second=><cursor>2)",vec![ordinary("second","integer",true)],"calc([first integer], [second integer])",vec![]),
        ("postgres","SELECT calc(\"MiXeD\"=><cursor>2,first=>1)",vec![ordinary("MiXeD","integer",false)],"calc(first integer, MiXeD integer)",vec![]),
        ("postgres","SELECT calc(\"ca\"\"lc\" /* x */ :=<cursor>2,first=>1)",vec![ordinary("ca\"lc","integer",false)],"calc(first integer, ca\"lc integer)",vec![]),
        ("postgres","SELECT calc(\"计算\"=><cursor>2,first=>1)",vec![ordinary("计算","integer",false)],"calc(first integer, 计算 integer)",vec![]),
        ("postgres","CALL calc(second=><cursor>NULL,first=>1)",vec![routine("procedure",vec![parameter("first","integer",false,Some("IN")),parameter("second","integer",false,Some("OUT"))])],"calc(first integer, OUT second integer)",vec![]),
        ("postgres","SELECT calc(second=><cursor>2,first=>1)",vec![routine("function",vec![parameter("out","text",false,Some("OUT")),parameter("first","integer",false,None),parameter("second","integer",false,None)])],"calc(first integer, second integer)",vec![]),
        ("postgres","SELECT calc(first=>1,VARIADIC second=><cursor>ARRAY[2,3]::integer[])",vec![routine("function",vec![parameter("first","integer",false,None),parameter("second","integer[]",false,Some("VARIADIC"))])],"calc(first integer, VARIADIC second integer[])",vec![]),
        ("postgres","WITH p AS (SELECT id AS value FROM users) SELECT calc(second=><cursor>1,first=>(SELECT MAX(value) FROM p))",vec![routine("function",vec![parameter("first","integer",false,None),parameter("second","bigint",false,None)]),routine("function",vec![parameter("first","bigint",false,None),parameter("second","integer",false,None)])],"calc(first bigint, second integer)",vec![]),
        ("PGSQL","SELECT \"app\".calc(SECOND /* , ) */ =><cursor>2,first=>(CASE WHEN TRUE THEN 1 ELSE 3 END))",vec![standard],"calc(first integer, second integer)",vec![]),
    ];
    assert_eq!(cases.len(), 12);
    for (index, (dialect, marked, functions, label, hints)) in cases.into_iter().enumerate() {
        let uri = format!("oxide://named/complex.{index}.mysql.sql");
        configure(&mut process, &uri, dialect, functions).await;
        let marked = format!("/* 😀 */\r\n/* 😀 */ {marked};");
        let cursor = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        semantic_check(
            &mut process,
            &uri,
            &sql,
            json!(sql_lsp::position::lsp_position_at_end(&sql[..cursor])),
            label,
            &hints,
        )
        .await;
    }
    for (index, marked) in [
        "SELECT calc(first=>1,first=><cursor>2)",
        "SELECT calc(first=>1,<cursor>2)",
        "SELECT calc(missing=><cursor>2)",
        "SELECT calc(\"SECOND\"=><cursor>2)",
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://named/invalid.{index}.mysql.sql");
        configure(
            &mut process,
            &uri,
            "postgres",
            vec![ordinary("second", "integer", false)],
        )
        .await;
        let cursor = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let help=process.request("textDocument/signatureHelp",json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..cursor])})).await;
        assert!(help["activeParameter"].is_null(), "{sql}: {help}");
        let hints=process.request("textDocument/inlayHint",json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":sql_lsp::position::lsp_position_at_end(&sql)}})).await;
        assert!(hints.as_array().unwrap().is_empty(), "{sql}: {hints}");
    }
    process.stop().await;
}

#[tokio::test]
async fn native_named_arguments_accept_name_edits_and_refresh_without_document_changes() {
    let mut process = LspProcess::start().await;
    let cases = [
        ("SELECT calc(first=>1,se<cursor>)", "second"),
        (
            "SELECT calc(first=>1,se<cursor>cond /* keep */ => 2)",
            "second",
        ),
        ("SELECT calc(first=>1,\"se<cursor>cond\" := 2)", "second"),
        ("SELECT calc(<cursor>)", "second"),
        ("SELECT calc(first=>1,\"计<cursor>算\" => 2)", "计算"),
        ("SELECT calc(first=>1,co<cursor>)", "co$st}"),
    ];
    assert_eq!(cases.len(), 6);
    for (index, (marked, name)) in cases.into_iter().enumerate() {
        let uri = format!("oxide://named/edit.{index}.mysql.sql");
        configure(
            &mut process,
            &uri,
            "postgres",
            vec![ordinary(name, "integer", true)],
        )
        .await;
        let marked = format!("/* 😀 */\r\n/* 😀 */ {marked};");
        let cursor = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let items = named_items(
            &mut process,
            &uri,
            json!(sql_lsp::position::lsp_position_at_end(&sql[..cursor])),
        )
        .await;
        let item = items
            .iter()
            .find(|item| item["label"] == name)
            .unwrap_or_else(|| panic!("{sql}: {items:?}"));
        assert!(
            !items.iter().any(|item| item["label"] == "first") || index == 3,
            "{items:?}"
        );
        let edit = &item["textEdit"];
        let start = byte_offset(&sql, &edit["range"]["start"]);
        let end = byte_offset(&sql, &edit["range"]["end"]);
        assert!(start <= cursor && end >= cursor, "{edit}");
        let replacement = edit["newText"]
            .as_str()
            .unwrap()
            .replace("$0", "2")
            .replace("\\$", "$")
            .replace("\\}", "}")
            .replace("\\\\", "\\");
        let edited = format!("{}{}{}", &sql[..start], replacement, &sql[end..]);
        assert!(edited.contains(&format!("\"{name}\"")), "{edited}");
        if sql.contains("/* keep */") {
            assert!(edited.contains("/* keep */ => 2"), "{edited}");
        }
        process
            .notify(
                "textDocument/didChange",
                json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":edited}]}),
            )
            .await;
        process
            .until(|message| {
                message["method"] == "textDocument/publishDiagnostics"
                    && message["params"]["uri"] == uri
                    && message["params"]["version"] == 2
            })
            .await;
        let value = edited.rfind('2').unwrap();
        semantic_check(
            &mut process,
            &uri,
            &edited,
            json!(sql_lsp::position::lsp_position_at_end(&edited[..value])),
            &format!("calc([first integer], [{name} integer])"),
            &[],
        )
        .await;
    }
    let uri = "oxide://named/refresh.mysql.sql";
    let sql = "SELECT calc(first=>1,se)";
    let rounds = [
        (vec![ordinary("second", "integer", false)], vec!["second"]),
        (
            vec![routine(
                "procedure",
                vec![
                    parameter("first", "integer", false, None),
                    parameter("second", "integer", false, None),
                ],
            )],
            vec![],
        ),
        (vec![ordinary("service", "integer", false)], vec!["service"]),
        (
            vec![routine(
                "function",
                vec![
                    parameter("other", "integer", false, None),
                    parameter("second", "integer", false, None),
                ],
            )],
            vec![],
        ),
        (
            vec![
                ordinary("second", "integer", false),
                ordinary("second", "text", false),
            ],
            vec!["second", "second"],
        ),
        (vec![ordinary("second", "integer", false)], vec!["second"]),
    ];
    assert_eq!(rounds.len(), 6);
    for (index, (functions, expected)) in rounds.into_iter().enumerate() {
        configure(&mut process, uri, "postgres", functions).await;
        if index == 0 {
            process.open(uri, sql).await;
        }
        let result = named_items(
            &mut process,
            uri,
            json!(sql_lsp::position::lsp_position_at_end(
                &sql[..sql.len() - 1]
            )),
        )
        .await;
        assert_eq!(
            result
                .iter()
                .map(|item| item["label"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected,
            "round {index}: {result:?}"
        );
    }
    process.stop().await;
}
