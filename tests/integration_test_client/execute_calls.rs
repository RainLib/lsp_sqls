use super::*;

fn routine(second: &str, ty: &str) -> Value {
    json!({"name":"calculate","routine_type":"procedure","return_type":"void","parameters":[
        {"name":"@first","data_type":"int","optional":false,"mode":"IN"},
        {"name":second,"data_type":ty,"optional":true,"mode":"OUT"}]})
}
async fn configure(p: &mut LspProcess, uri: &str, dialect: &str, routines: Vec<Value>) {
    p.notify("workspace/didChangeConfiguration", json!({"settings":{
        "schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","catalog":"db","tables":[],"functions":routines}],
        "fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):dialect}
    }})).await;
}
async fn items(p: &mut LspProcess, uri: &str, position: Value) -> Vec<Value> {
    let result = p
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
// Independent UTF-16 edit oracle; do not validate server ranges with its own helper.
fn byte_offset(text: &str, pos: &Value) -> usize {
    let (mut line, mut column) = (0, 0);
    for (at, ch) in text.char_indices() {
        if line == pos["line"].as_u64().unwrap() && column == pos["character"].as_u64().unwrap() {
            return at;
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
            pos["line"].as_u64().unwrap(),
            pos["character"].as_u64().unwrap()
        )
    );
    text.len()
}
async fn no_errors(p: &mut LspProcess, uri: &str) {
    let result = p
        .request(
            "textDocument/diagnostic",
            json!({"textDocument":{"uri":uri}}),
        )
        .await;
    assert!(
        result["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["severity"] != 1),
        "{result}"
    );
}
#[tokio::test]
async fn native_execute_edits_preserve_arguments_and_use_real_unparenthesized_signatures() {
    let mut p = LspProcess::start().await;
    let cases = [
        ("sqlserver", "/*😀*/ EXEC app.ca<cursor>lculate 1, @v OUTPUT;", "calculate", "[calculate]", "/*😀*/ EXEC app.[calculate] 1, @v OUTPUT;"),
        ("mssql", "SELECT 1;\r\nEXECUTE @s = [db].[app].[ca<cursor>lculate] @second = @v OUT, @first = 1;", "calculate", "[calculate]", "SELECT 1;\r\nEXECUTE @s = [db].[app].[calculate] @second = @v OUT, @first = 1;"),
        ("tsql", "EXEC app.calculate 1, @se<cursor>cond /*preserve*/ = @v OUTPUT;", "@second", "@second", "EXEC app.calculate 1, @second /*preserve*/ = @v OUTPUT;"),
        ("sqlserver", "EXEC app.calculate @se<cursor>", "@second", "@second = $0", "EXEC app.calculate @second = @v, @first = 1;"),
        ("sqlserver", "EXEC ca<cursor>", "calculate", "[calculate]", "EXEC [calculate] 1, @v OUTPUT;"),
        ("sqlserver", "EXEC <cursor>", "calculate", "[calculate]", "EXEC [calculate] 1, @v OUTPUT;"),
    ];
    for (index, (dialect, marked, label, insertion, expected)) in cases.iter().enumerate() {
        let uri = format!("oxide://execute/edit-{index}.mysql.sql");
        let mut scalar = routine("@second", "nvarchar");
        scalar["name"] = json!("callable_function");
        scalar["routine_type"] = json!("function");
        configure(
            &mut p,
            &uri,
            dialect,
            vec![routine("@second", "nvarchar"), scalar],
        )
        .await;
        let offset = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        p.open(&uri, &sql).await;
        let results = items(
            &mut p,
            &uri,
            serde_json::to_value(sql_lsp::position::lsp_position_at_end(&marked[..offset]))
                .unwrap(),
        )
        .await;
        let chosen = results
            .iter()
            .find(|item| item["label"] == *label)
            .expect("EXEC recommendation");
        assert_eq!(chosen["textEdit"]["newText"], *insertion, "{results:?}");
        let edit = &chosen["textEdit"];
        let mut accepted = sql.clone();
        accepted.replace_range(
            byte_offset(&sql, &edit["range"]["start"])..byte_offset(&sql, &edit["range"]["end"]),
            insertion,
        );
        if accepted.contains("$0") {
            accepted = accepted.replace("$0", "@v, @first = 1;");
        } else if index >= 4 {
            accepted.push_str(" 1, @v OUTPUT;");
        }
        assert_eq!(accepted, *expected);
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
        let at = accepted.find("@v").unwrap();
        let help = p.request("textDocument/signatureHelp", json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&accepted[..at])})).await;
        assert_eq!(help["activeParameter"], 1, "{accepted}: {help}");
        assert_eq!(
            help["signatures"][0]["parameters"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let hints = p.request("textDocument/inlayHint",json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":sql_lsp::position::lsp_position_at_end(&accepted)}})).await;
        let count = if index == 0 || index >= 4 {
            2
        } else if index == 2 {
            1
        } else {
            0
        };
        assert_eq!(
            hints.as_array().unwrap().len(),
            count,
            "{accepted}: {hints}"
        );
        no_errors(&mut p, &uri).await;
    }
    p.stop().await;
}

#[tokio::test]
async fn native_execute_metadata_refresh_rebinds_without_document_edits() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://execute/refresh.mysql.sql";
    let marked = "EXEC app.calculate @se";
    let position = serde_json::to_value(sql_lsp::position::lsp_position_at_end(marked)).unwrap();
    configure(
        &mut p,
        uri,
        "sqlserver",
        vec![routine("@second", "nvarchar")],
    )
    .await;
    p.open(uri, marked).await;
    for (name, ty) in [
        ("@second", "nvarchar"),
        ("@second_new", "bigint"),
        ("@second", "int"),
        ("@second", "nvarchar"),
    ] {
        configure(&mut p, uri, "sqlserver", vec![routine(name, ty)]).await;
        let result = items(&mut p, uri, position.clone()).await;
        let named = result
            .iter()
            .filter(|i| {
                i["detail"]
                    .as_str()
                    .is_some_and(|v| v.starts_with("Parameter:"))
            })
            .collect::<Vec<_>>();
        assert_eq!(named.len(), 1, "{result:?}");
        assert_eq!(named[0]["label"], name);
        assert!(named[0]["detail"].as_str().unwrap().contains(ty));
    }
    configure(&mut p, uri, "sqlserver", vec![]).await;
    assert!(!items(&mut p, uri, position.clone())
        .await
        .iter()
        .any(|i| i["detail"]
            .as_str()
            .is_some_and(|v| v.starts_with("Parameter:"))));
    configure(
        &mut p,
        uri,
        "sqlserver",
        vec![routine("@second", "nvarchar")],
    )
    .await;
    assert!(items(&mut p, uri, position)
        .await
        .iter()
        .any(|i| i["label"] == "@second"));
    p.stop().await;
}

#[tokio::test]
async fn native_execute_complex_literals_defaults_and_real_errors_keep_dialect_routing() {
    let mut p = LspProcess::start().await;
    for (index, (dialect, sql, errors)) in [
        (
            "sqlserver",
            "/*😀*/ EXECUTE @s = [db].[app].[calculate] @second = N'a,b;WITH', @first = -1;",
            false,
        ),
        (
            "mssql",
            "EXEC app.calculate @first = DEFAULT WITH RECOMPILE;",
            false,
        ),
        (
            "tsql",
            "EXEC app.calculate 1, @value OUTPUT; SELECT 1;",
            false,
        ),
        ("sqlserver", "EXEC app.calculate @first = ;", true),
        ("sqlserver", "EXEC app.calculate 1 OUTPUT;", true),
        ("sqlserver", "EXEC app.calculate (1 + 2);", true),
        ("postgres", "EXEC app.calculate 1;", true),
        ("mysql", "EXEC app.calculate 1;", true),
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://execute/grammar-{index}.mysql.sql");
        configure(&mut p, &uri, dialect, vec![routine("@second", "nvarchar")]).await;
        p.open(&uri, sql).await;
        let diagnostics = p
            .request(
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":uri}}),
            )
            .await;
        assert_eq!(
            diagnostics["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["severity"] == 1),
            *errors,
            "{sql}: {diagnostics}"
        );
    }
    for (index, sql) in [
        "-- EXEC app.calculate 1, ",
        "SELECT 'EXEC app.calculate 1, ';",
        "EXEC ('EXEC app.calculate 1, ');",
        "EXEC @module 1, ",
        "EXEC SUM 1,",
        "CALL SUM(1,",
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://execute/noise-{index}.sql");
        configure(
            &mut p,
            &uri,
            "sqlserver",
            vec![routine("@second", "nvarchar")],
        )
        .await;
        p.open(&uri, sql).await;
        let help = p.request("textDocument/signatureHelp", json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(sql)})).await;
        assert!(help.is_null(), "{sql}: {help}");
    }
    p.stop().await;
}
