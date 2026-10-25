use super::*;

fn parameter(name: &str, ty: &str, mode: Option<&str>) -> Value {
    json!({"name":name,"data_type":ty,"optional":false,"mode":mode})
}
fn function(kind: &str, parameters: Vec<Value>) -> Value {
    json!({"name":"calc","routine_type":kind,"parameters":parameters,"return_type":"integer","description":null})
}
async fn configure(process: &mut LspProcess, uri: &str, dialect: &str, functions: Vec<Value>) {
    let id = "aaaaaaaa-1111-4111-8111-111111111111";
    process.notify("workspace/didChangeConfiguration",json!({"settings":{"schemas":[{"id":id,"database":"app","catalog":"db","tables":[],"functions":functions}],"fileSchemas":{(uri):id},"fileDialects":{(uri):dialect}}})).await;
}
async fn hints(process: &mut LspProcess, uri: &str, sql: &str) -> Value {
    process.request("textDocument/inlayHint",json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":sql_lsp::position::lsp_position_at_end(sql)}})).await
}
async fn no_errors(process: &mut LspProcess, uri: &str, sql: &str) {
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
async fn native_routine_modes_keep_output_variadic_signatures_labels_and_real_diagnostics() {
    let mut process = LspProcess::start().await;
    let cases = [
        (
            "postgres",
            "function",
            vec![
                parameter("result", "integer", Some("OUT")),
                parameter("value", "integer", Some("IN")),
            ],
            "SELECT calc(<cursor>1)",
            "calc(value integer)",
            Some(0),
            vec!["value:"],
        ),
        (
            "postgres",
            "function",
            vec![parameter("value", "integer", Some("b"))],
            "SELECT calc(<cursor>1)",
            "calc(INOUT value integer)",
            Some(0),
            vec!["value:"],
        ),
        (
            "postgres",
            "function",
            vec![
                parameter("row_name", "text", Some("TABLE")),
                parameter("value", "integer", None),
            ],
            "SELECT calc(<cursor>1)",
            "calc(value integer)",
            Some(0),
            vec!["value:"],
        ),
        (
            "postgres",
            "function",
            vec![parameter("result", "integer", Some("OUT"))],
            "SELECT calc(<cursor>)",
            "calc()",
            None,
            vec![],
        ),
        (
            "postgres",
            "function",
            vec![parameter("items", "integer[]", Some("VARIADIC"))],
            "SELECT calc(1,2,<cursor>3)",
            "calc(VARIADIC items integer[])",
            Some(0),
            vec!["items:", "items:", "items:"],
        ),
        (
            "postgres",
            "function",
            vec![
                parameter("prefix", "integer", None),
                parameter("items", "integer[]", Some("VARIADIC")),
            ],
            "SELECT calc(1,2,<cursor>3)",
            "calc(prefix integer, VARIADIC items integer[])",
            Some(1),
            vec!["prefix:", "items:", "items:"],
        ),
        (
            "postgres",
            "function",
            vec![parameter("items", "integer[]", Some("VARIADIC"))],
            "SELECT calc(VARIADIC ARRAY[1,2]::integer[]<cursor>)",
            "calc(VARIADIC items integer[])",
            Some(0),
            vec!["items:"],
        ),
        (
            "postgres",
            "function",
            vec![parameter("items", "integer[]", Some("VARIADIC"))],
            "SELECT calc(<cursor>VARIADIC ARRAY[1,2]::integer[])",
            "calc(VARIADIC items integer[])",
            Some(0),
            vec!["items:"],
        ),
        (
            "postgres",
            "function",
            vec![
                parameter("prefix", "integer", None),
                parameter("items", "integer[]", Some("VARIADIC")),
            ],
            "SELECT calc(1,<cursor>VARIADIC ARRAY[]::integer[])",
            "calc(prefix integer, VARIADIC items integer[])",
            Some(1),
            vec!["prefix:", "items:"],
        ),
        (
            "postgres",
            "procedure",
            vec![
                parameter("value", "integer", Some("IN")),
                parameter("result", "integer", Some("OUT")),
            ],
            "CALL calc(1,<cursor>NULL)",
            "calc(value integer, OUT result integer)",
            Some(1),
            vec!["value:", "result:"],
        ),
        (
            "mysql",
            "procedure",
            vec![
                parameter("value", "integer", Some("IN")),
                parameter("result", "integer", Some("OUT")),
            ],
            "CALL calc(1,<cursor>@result)",
            "calc(value integer, OUT result integer)",
            Some(1),
            vec!["value:", "result:"],
        ),
        (
            "postgres",
            "function",
            vec![parameter("value", "integer", None)],
            "SELECT calc(<cursor>1)",
            "calc(value integer)",
            Some(0),
            vec!["value:"],
        ),
    ];
    assert_eq!(cases.len(), 12);
    for (index, (dialect, kind, parameters, marked, label, active, expected_hints)) in
        cases.into_iter().enumerate()
    {
        let uri = format!("oxide://routine/modes.{index}.mysql.sql");
        let mut functions = vec![];
        if marked.contains("VARIADIC") && parameters.len() == 1 {
            functions.push(function(
                "function",
                vec![parameter("ordinary_array", "integer[]", None)],
            ));
        }
        functions.push(function(kind, parameters));
        configure(&mut process, &uri, dialect, functions).await;
        let marked = format!("/* 😀 */\r\n/* 😀 */ {marked};");
        let offset = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let help=process.request("textDocument/signatureHelp",json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..offset])})).await;
        assert_eq!(help["signatures"][0]["label"], label, "{sql}: {help}");
        assert_eq!(help["activeParameter"], json!(active), "{sql}: {help}");
        let result = hints(&mut process, &uri, &sql).await;
        let actual = result
            .as_array()
            .unwrap()
            .iter()
            .map(|hint| hint["label"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected_hints, "{sql}: {result}");
        if marked.contains("VARIADIC") {
            assert_eq!(
                result.as_array().unwrap().last().unwrap()["tooltip"],
                "Parameter type: integer[]"
            );
        }
        no_errors(&mut process, &uri, &sql).await;
    }
    let rejected = [
        ("postgres", "SELECT calc(VARIADIC ARRAY[1],2);"),
        (
            "postgres",
            "SELECT calc(VARIADIC ARRAY[1],VARIADIC ARRAY[2]);",
        ),
        ("mysql", "SELECT calc(VARIADIC ARRAY[1]);"),
    ];
    assert_eq!(rejected.len(), 3);
    for (index, (dialect, sql)) in rejected.into_iter().enumerate() {
        let uri = format!("oxide://routine/rejected.{index}.sql");
        configure(&mut process, &uri, dialect, vec![]).await;
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
                .any(|item| item["severity"] == 1 && item["code"] == "UNSUPPORTED_DIALECT_SYNTAX"),
            "{sql}: {diagnostics}"
        );
    }
    process.stop().await;
}

#[tokio::test]
async fn native_routine_modes_refresh_signature_and_inlays_when_only_modes_change() {
    let mut process = LspProcess::start().await;
    let uri = "oxide://routine/mode-refresh.mysql.sql";
    let sql = "/* 😀 */\r\nSELECT calc(1, 2);";
    let value = sql.rfind('2').unwrap();
    let rounds = [
        (
            vec![
                parameter("a", "integer", None),
                parameter("b", "integer", None),
            ],
            2,
            1,
            vec!["a:", "b:"],
        ),
        (
            vec![
                parameter("result", "integer", Some("OUT")),
                parameter("a", "integer", Some("IN")),
                parameter("b", "integer", Some("INOUT")),
            ],
            2,
            1,
            vec!["a:", "b:"],
        ),
        (
            vec![
                parameter("a", "integer", None),
                parameter("tail", "integer[]", Some("VARIADIC")),
            ],
            2,
            1,
            vec!["a:", "tail:"],
        ),
        (
            vec![
                parameter("result", "integer", Some("OUT")),
                parameter("tail", "integer[]", Some("VARIADIC")),
            ],
            1,
            0,
            vec!["tail:", "tail:"],
        ),
        (
            vec![
                parameter("a", "integer", Some("INOUT")),
                parameter("b", "integer", Some("INOUT")),
            ],
            2,
            1,
            vec!["a:", "b:"],
        ),
        (
            vec![
                parameter("a", "integer", None),
                parameter("b", "integer", None),
            ],
            2,
            1,
            vec!["a:", "b:"],
        ),
    ];
    assert_eq!(rounds.len(), 6);
    for (round, (parameters, count, active, labels)) in rounds.into_iter().enumerate() {
        configure(
            &mut process,
            uri,
            "postgres",
            vec![function("function", parameters)],
        )
        .await;
        if round == 0 {
            process.open(uri, sql).await;
        }
        let help=process.request("textDocument/signatureHelp",json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..value])})).await;
        assert_eq!(
            help["signatures"][0]["parameters"]
                .as_array()
                .unwrap()
                .len(),
            count,
            "{round}: {help}"
        );
        assert_eq!(help["activeParameter"], active);
        let result = hints(&mut process, uri, sql).await;
        let actual = result
            .as_array()
            .unwrap()
            .iter()
            .map(|hint| hint["label"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(actual, labels, "{round}: {result}");
        assert_eq!(
            result[1]["position"],
            json!(sql_lsp::position::lsp_position_at_end(&sql[..value]))
        );
    }
    no_errors(&mut process, uri, sql).await;
    process.stop().await;
}
