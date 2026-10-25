use super::*;

fn routine(name: &str, kind: &str, ty: &str) -> Value {
    json!({"name":name,"routine_type":kind,"return_type":"void","parameters":[
        {"name":"value","data_type":ty,"optional":false,"mode":"IN"},
        {"name":"result","data_type":"integer","optional":false,"mode":"OUT"}]})
}
async fn configure(process: &mut LspProcess, uri: &str, dialect: &str, functions: Vec<Value>) {
    process.notify("workspace/didChangeConfiguration", json!({"settings":{
        "schemas":[
            {"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","catalog":"db","tables":[],"functions":functions},
            {"id":"bbbbbbbb-1111-4111-8111-111111111111","database":"analytics","catalog":"db","tables":[],"functions":[routine("remote","procedure","integer")]}
        ],
        "fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):dialect}
    }})).await;
}
async fn complete(process: &mut LspProcess, uri: &str, position: Value) -> Vec<Value> {
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
        .clone()
}
// Independent UTF-16 edit oracle: never use the server's offset conversion to
// validate the server's TextEdit range (especially after astral characters).
fn byte_offset(text: &str, position: &Value) -> usize {
    let (mut line, mut column) = (0, 0);
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
    assert_eq!(line, position["line"].as_u64().unwrap());
    assert_eq!(column, position["character"].as_u64().unwrap());
    text.len()
}
async fn replace(process: &mut LspProcess, uri: &str, sql: &str) {
    process
        .notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":sql}]}),
        )
        .await;
    process
        .until(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
                && message["params"]["version"] == 2
        })
        .await;
}

#[tokio::test]
async fn native_call_completion_applies_quoted_edits_then_keeps_signature_and_inlays() {
    let mut process = LspProcess::start().await;
    let cases = [
        (
            "postgres",
            "CALL ca<cursor>",
            "calculate",
            "CALL \"calculate\"(1,NULL)",
        ),
        (
            "postgres",
            "CALL app.ca<cursor>lculate /* x */ (1,NULL)",
            "calculate",
            "CALL app.\"calculate\" /* x */ (1,NULL)",
        ),
        (
            "postgres",
            "CALL app.\"Ca<cursor>lculate\"(1,NULL)",
            "Calculate",
            "CALL app.\"Calculate\"(1,NULL)",
        ),
        (
            "postgres",
            "CALL app.\"ca\"<cursor>\"lc\"(1,NULL)",
            "ca\"lc",
            "CALL app.\"ca\"\"lc\"(1,NULL)",
        ),
        (
            "mysql",
            "CALL `app`.`cal<cursor>c``x`(1,@result)",
            "calc`x",
            "CALL `app`.`calc``x`(1,@result)",
        ),
        (
            "postgres",
            "CALL app.\"calc<cursor>.with.dot\"(1,NULL)",
            "calc.with.dot",
            "CALL app.\"calc.with.dot\"(1,NULL)",
        ),
        (
            "postgres",
            "CALL db.app.ca<cursor>(1,NULL)",
            "calculate",
            "CALL db.app.\"calculate\"(1,NULL)",
        ),
        (
            "postgres",
            "CALL analytics.re<cursor>(1,NULL)",
            "remote",
            "CALL analytics.\"remote\"(1,NULL)",
        ),
        (
            "postgres",
            "CALL app.<cursor>(1,NULL)",
            "calculate",
            "CALL app.\"calculate\"(1,NULL)",
        ),
        (
            "postgres",
            "CALL <cursor>",
            "calculate",
            "CALL \"calculate\"(1,NULL)",
        ),
        (
            "postgres",
            "CALL \"ca<cursor>\r\n;SELECT 1",
            "calculate",
            "CALL \"calculate\"(1,NULL)\r\n;SELECT 1",
        ),
        (
            "postgres",
            "CALL ca  <cursor> /* keep */(1,NULL)",
            "calculate",
            "CALL \"calculate\" /* keep */(1,NULL)",
        ),
    ];
    assert_eq!(cases.len(), 12);
    for (index, (dialect, marked, name, expected)) in cases.into_iter().enumerate() {
        let uri = format!("oxide://call/completion.{index}.mysql.sql");
        configure(
            &mut process,
            &uri,
            dialect,
            vec![
                routine(name, "procedure", "integer"),
                routine("calculate_function", "function", "integer"),
            ],
        )
        .await;
        let marked = format!("/* 😀 */\r\n/* 😀 */ {marked};");
        let cursor = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let position = json!(sql_lsp::position::lsp_position_at_end(&sql[..cursor]));
        let items = complete(&mut process, &uri, position).await;
        assert!(
            items.iter().all(|item| item["detail"]
                .as_str()
                .is_some_and(|detail| detail.starts_with("Procedure:"))),
            "{sql}: {items:?}"
        );
        let item = items
            .iter()
            .find(|item| item["label"] == name)
            .unwrap_or_else(|| panic!("{sql}: {items:?}"));
        let edit = &item["textEdit"];
        let start = byte_offset(&sql, &edit["range"]["start"]);
        let end = byte_offset(&sql, &edit["range"]["end"]);
        assert!(start <= cursor && end >= cursor, "{sql}: {edit}");
        let replacement = edit["newText"].as_str().unwrap().replace(
            "$0",
            if dialect == "mysql" {
                "1,@result"
            } else {
                "1,NULL"
            },
        );
        let edited = format!("{}{}{}", &sql[..start], replacement, &sql[end..]);
        assert_eq!(edited, format!("/* 😀 */\r\n/* 😀 */ {expected};"));
        replace(&mut process, &uri, &edited).await;
        let argument = edited
            .find(if dialect == "mysql" {
                "@result"
            } else {
                "NULL"
            })
            .unwrap();
        let help=process.request("textDocument/signatureHelp",json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&edited[..argument])})).await;
        assert_eq!(
            help["signatures"][0]["label"],
            format!("{name}(value integer, OUT result integer)"),
            "{edited}: {help}"
        );
        assert_eq!(help["activeParameter"], 1, "{help}");
        let hints=process.request("textDocument/inlayHint",json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":sql_lsp::position::lsp_position_at_end(&edited)}})).await;
        assert_eq!(
            hints
                .as_array()
                .unwrap()
                .iter()
                .map(|hint| hint["label"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["value:", "result:"],
            "{edited}: {hints}"
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
            "{edited}: {diagnostics}"
        );
    }
    process.stop().await;
}

#[tokio::test]
async fn native_call_completion_refreshes_kinds_overloads_and_names_without_sql_changes() {
    let mut process = LspProcess::start().await;
    let uri = "oxide://call/refresh.mysql.sql";
    let sql = "/* 😀 */\r\nCALL cal";
    let rounds = [
        (
            vec![routine("calc_old", "procedure", "integer")],
            vec!["calc_old"],
        ),
        (vec![routine("calc_old", "function", "integer")], vec![]),
        (
            vec![
                routine("calc_new", "procedure", "integer"),
                routine("calc_new", "procedure", "text"),
                routine("calc_new", "procedure", "text"),
            ],
            vec!["calc_new", "calc_new"],
        ),
        (vec![], vec![]),
        (
            vec![routine("calculate", "procedure", "integer")],
            vec!["calculate"],
        ),
        (
            vec![routine("calc_old", "procedure", "integer")],
            vec!["calc_old"],
        ),
    ];
    assert_eq!(rounds.len(), 6);
    for (index, (functions, expected)) in rounds.into_iter().enumerate() {
        configure(&mut process, uri, "postgres", functions).await;
        if index == 0 {
            process.open(uri, sql).await;
        }
        let items = complete(
            &mut process,
            uri,
            json!(sql_lsp::position::lsp_position_at_end(sql)),
        )
        .await;
        assert_eq!(
            items
                .iter()
                .map(|item| item["label"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected,
            "round {index}: {items:?}"
        );
        if items.len() == 2 {
            assert_ne!(items[0]["detail"], items[1]["detail"]);
        }
    }
    for (index, marked) in [
        "CALL missing.cal<cursor>",
        "SELECT 'CALL cal<cursor>'",
        "-- CALL cal<cursor>",
        "CALL calc_old('cal<cursor>')",
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://call/negative.{index}.mysql.sql");
        configure(
            &mut process,
            &uri,
            "postgres",
            vec![routine("calc_old", "procedure", "integer")],
        )
        .await;
        let offset = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let items = complete(
            &mut process,
            &uri,
            json!(sql_lsp::position::lsp_position_at_end(&sql[..offset])),
        )
        .await;
        assert!(
            !items.iter().any(|item| item["detail"]
                .as_str()
                .is_some_and(|detail| detail.starts_with("Procedure:"))),
            "{sql}: {items:?}"
        );
    }
    process.stop().await;
}
