use super::*;

pub(super) fn routine(optional: bool, mode: &str) -> Value {
    json!({"name":"calculate","routine_type":"procedure","return_type":"void","parameters":[
        {"name":"@first","data_type":"int","optional":false,"mode":"IN"},
        {"name":"@second","data_type":"nvarchar","optional":optional,"mode":mode}]})
}
pub(super) async fn configure(p: &mut LspProcess, uri: &str, dialect: &str, routines: Vec<Value>) {
    p.notify("workspace/didChangeConfiguration",json!({"settings":{
        "schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","catalog":"db","tables":[{"name":"users","columns":[{"name":"id","data_type":"int","nullable":false}]}],"functions":routines}],
        "fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):dialect}
    }})).await;
}
pub(super) async fn complete(p: &mut LspProcess, uri: &str, position: Value) -> Vec<Value> {
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
pub(super) fn offset(text: &str, pos: &Value) -> usize {
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
pub(super) async fn errors(p: &mut LspProcess, uri: &str) -> Vec<Value> {
    p.request(
        "textDocument/diagnostic",
        json!({"textDocument":{"uri":uri}}),
    )
    .await["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["severity"] == 1)
        .cloned()
        .collect()
}

#[tokio::test]
async fn native_execute_values_accept_plain_edits_and_validate_real_declarations() {
    let mut p = LspProcess::start().await;
    let cases=[
        ("sqlserver","/*😀*/ DECLARE @value int=COALESCE(1,2);\r\n/*😀*/ EXEC app.calculate @first=@va<cursor>;","@value","/*😀*/ DECLARE @value int=COALESCE(1,2);\r\n/*😀*/ EXEC app.calculate @first=@value;",false),
        ("mssql","DECLARE @a decimal(18,2)=COALESCE(1,2), @message AS nvarchar(max)=CONCAT(N'a,b',N'c'); EXEC app.calculate 1, @second=@mes<cursor>sage;","@message","DECLARE @a decimal(18,2)=COALESCE(1,2), @message AS nvarchar(max)=CONCAT(N'a,b',N'c'); EXEC app.calculate 1, @second=@message;",false),
        ("tsql","EXEC app.calculate @first = NU<cursor>LL;","NULL","EXEC app.calculate @first = NULL;",false),
        ("sqlserver","EXEC app.calculate 1, @second = DE<cursor>FAULT;","DEFAULT","EXEC app.calculate 1, @second = DEFAULT;",false),
        ("sqlserver","DECLARE @sink nvarchar(max); EXEC app.calculate 1, @second=@sink O<cursor>;","OUTPUT","DECLARE @sink nvarchar(max); EXEC app.calculate 1, @second=@sink OUTPUT;",true),
        ("sqlserver","EXEC app.calculate @first = <cursor> /*keep*/;","NULL","EXEC app.calculate @first = NULL /*keep*/;",true),
    ];
    for (index, (dialect, marked, label, expected, initial_error)) in cases.iter().enumerate() {
        let uri = format!("oxide://execute/value-edit-{index}.mysql.sql");
        configure(&mut p, &uri, dialect, vec![routine(true, "OUT")]).await;
        let cursor = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        p.open(&uri, &sql).await;
        assert_eq!(
            !errors(&mut p, &uri).await.is_empty(),
            *initial_error,
            "{sql}"
        );
        let items = complete(
            &mut p,
            &uri,
            serde_json::to_value(sql_lsp::position::lsp_position_at_end(&marked[..cursor]))
                .unwrap(),
        )
        .await;
        assert!(
            items.iter().all(|item| item["kind"] == 6
                || ["NULL", "DEFAULT", "OUTPUT"]
                    .iter()
                    .any(|label| item["label"] == *label)),
            "{sql}: {items:?}"
        );
        let item = items
            .iter()
            .find(|item| item["label"] == *label)
            .expect("contextual value");
        assert_eq!(item["insertTextFormat"], 1);
        let edit = &item["textEdit"];
        let mut accepted = sql.clone();
        accepted.replace_range(
            offset(&sql, &edit["range"]["start"])..offset(&sql, &edit["range"]["end"]),
            edit["newText"].as_str().unwrap(),
        );
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
        assert!(errors(&mut p, &uri).await.is_empty(), "{accepted}");
    }
    p.stop().await;
}

#[tokio::test]
async fn native_execute_value_modes_refresh_without_sql_edits() {
    let mut p = LspProcess::start().await;
    for (index, (sql, label)) in [
        ("EXEC app.calculate 1, @second = DE", "DEFAULT"),
        (
            "DECLARE @sink int; EXEC app.calculate 1, @second=@sink O",
            "OUTPUT",
        ),
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://execute/value-refresh-{index}.mysql.sql");
        configure(&mut p, &uri, "sqlserver", vec![routine(true, "OUT")]).await;
        p.open(&uri, sql).await;
        let position = serde_json::to_value(sql_lsp::position::lsp_position_at_end(sql)).unwrap();
        for (optional, mode, exists) in [
            (true, "OUT", true),
            (false, "IN", false),
            (true, "INOUT", true),
            (false, "IN", false),
            (true, "OUT", true),
        ] {
            configure(&mut p, &uri, "sqlserver", vec![routine(optional, mode)]).await;
            let result = complete(&mut p, &uri, position.clone()).await;
            assert_eq!(
                result.iter().any(|item| item["label"] == *label),
                exists,
                "{sql}: {result:?}"
            );
        }
        configure(
            &mut p,
            &uri,
            "sqlserver",
            vec![routine(true, "OUT"), routine(false, "IN")],
        )
        .await;
        assert!(complete(&mut p, &uri, position.clone()).await.is_empty());
        configure(&mut p, &uri, "sqlserver", vec![]).await;
        assert!(complete(&mut p, &uri, position).await.is_empty());
    }
    p.stop().await;
}

#[tokio::test]
async fn native_execute_value_scope_caps_and_negative_sites_do_not_leak_generic_candidates() {
    let mut p = LspProcess::start().await;
    for(index,(marked,labels))in [
        ("DECLARE @old int;\nGO 2 -- reset\nDECLARE @current int; EXEC app.calculate @first=@<cursor>; DECLARE @future int;",vec!["@current"]),
        ("SELECT N'DECLARE @fake int'; /*DECLARE @fake2 int;*/ EXEC app.calculate @first=@<cursor>;",vec![]),
        ("DECLARE @value int; EXEC app.calculate @first=1, @<cursor>;",vec!["@second"]),
        ("EXEC app.calculate @first=CO<cursor>;",vec![]),
        ("EXEC app.calculate @first=@v O<cursor>;",vec![]),
        ("EXEC app.calculate 1,@second=1 O<cursor>;",vec![]),
    ].iter().enumerate(){
        let uri=format!("oxide://execute/value-scope-{index}.mysql.sql");configure(&mut p,&uri,"sqlserver",vec![routine(true,"OUT")]).await;
        let cursor=marked.find("<cursor>").unwrap();p.open(&uri,&marked.replace("<cursor>","")).await;
        let result=complete(&mut p,&uri,serde_json::to_value(sql_lsp::position::lsp_position_at_end(&marked[..cursor])).unwrap()).await;
        assert_eq!(result.iter().map(|i|i["label"].as_str().unwrap()).collect::<Vec<_>>(),*labels,"{marked}: {result:?}");
    }
    let declarations = (0..1002)
        .map(|n| format!("DECLARE @pick{n:04} int; "))
        .collect::<String>();
    let sql = format!("{declarations}DECLARE @pick nvarchar(max); EXEC app.calculate @first=@pick");
    let uri = "oxide://execute/value-cap.mysql.sql";
    configure(&mut p, uri, "sqlserver", vec![routine(true, "OUT")]).await;
    p.open(uri, &sql).await;
    let result = complete(
        &mut p,
        uri,
        serde_json::to_value(sql_lsp::position::lsp_position_at_end(&sql)).unwrap(),
    )
    .await;
    assert_eq!(result.len(), 100);
    assert_eq!(result[0]["label"], "@pick");
    let response=p.request("textDocument/completion",json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql)})).await;
    assert_eq!(response["isIncomplete"], true);
    for (version, prefix, count) in [(2, "@pick00", 100), (3, "@pick1001", 1)] {
        let changed = format!(
            "{declarations}DECLARE @pick nvarchar(max); EXEC app.calculate @first={prefix}"
        );
        p.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":changed}]})).await;
        p.until(|m| {
            m["method"] == "textDocument/publishDiagnostics"
                && m["params"]["uri"] == uri
                && m["params"]["version"] == version
        })
        .await;
        let response=p.request("textDocument/completion",json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&changed)})).await;
        assert_ne!(response["isIncomplete"], true);
        let result = response
            .as_array()
            .or_else(|| response["items"].as_array())
            .unwrap();
        assert_eq!(result.len(), count);
        if count == 1 {
            assert_eq!(result[0]["label"], "@pick1001");
        }
    }
    assert!(errors(&mut p, uri).await.is_empty());
    p.stop().await;
}

#[tokio::test]
async fn native_tsql_unicode_variable_edits_keep_utf16_names_and_full_diagnostics() {
    let mut p = LspProcess::start().await;
    let mut names = ["@金额", "@计数$#", "@a@b", "@#临时", "@1", "@Δ٥"]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    names.push(format!("@{}", "值".repeat(127)));
    for (index, name) in names.iter().enumerate() {
        let prefix = name.chars().take(2).collect::<String>();
        let marked=format!("/*😀*/ DECLARE {name} int=COALESCE(1,2);\r\n/*😀*/ EXEC app.calculate @first={prefix}<cursor>;");
        let uri = format!("oxide://unicode/variable-{index}.mysql.sql");
        configure(
            &mut p,
            &uri,
            ["sqlserver", "mssql", "tsql"][index % 3],
            vec![routine(true, "OUT")],
        )
        .await;
        let cursor = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        p.open(&uri, &sql).await;
        let result = complete(
            &mut p,
            &uri,
            serde_json::to_value(sql_lsp::position::lsp_position_at_end(&marked[..cursor]))
                .unwrap(),
        )
        .await;
        assert_eq!(result.len(), 1, "{name}: {result:?}");
        assert_eq!(result[0]["label"], *name);
        assert_eq!(result[0]["kind"], 6);
        let edit = &result[0]["textEdit"];
        let mut accepted = sql.clone();
        accepted.replace_range(
            offset(&sql, &edit["range"]["start"])..offset(&sql, &edit["range"]["end"]),
            edit["newText"].as_str().unwrap(),
        );
        assert!(accepted.ends_with(&format!("@first={name};")));
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
        assert!(errors(&mut p, &uri).await.is_empty(), "{accepted}");
    }
    let uri = "oxide://unicode/named.mysql.sql";
    let mut declared = routine(true, "OUT");
    declared["parameters"][0]["name"] = json!("@输入$");
    declared["parameters"][1]["name"] = json!("@输出#");
    configure(&mut p, uri, "sqlserver", vec![declared]).await;
    let marked="DECLARE @状态$ int,@结果# int; EXEC @状态$ = app.calculate @输<cursor>出# = @结果# OUTPUT, @输入$=1;";
    let cursor = marked.find("<cursor>").unwrap();
    let sql = marked.replace("<cursor>", "");
    p.open(uri, &sql).await;
    assert!(errors(&mut p, uri).await.is_empty());
    let result = complete(
        &mut p,
        uri,
        serde_json::to_value(sql_lsp::position::lsp_position_at_end(&marked[..cursor])).unwrap(),
    )
    .await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["label"], "@输出#");
    assert_eq!(result[0]["textEdit"]["newText"], "@输出#");
    let before = sql.split("@结果# OUTPUT").next().unwrap();
    let help=p.request("textDocument/signatureHelp",json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(before)})).await;
    assert_eq!(help["activeParameter"], 1);
    assert!(help["signatures"][0]["label"]
        .as_str()
        .unwrap()
        .contains("OUT @输出#"));
    p.stop().await;
}

#[tokio::test]
async fn native_tsql_unicode_opaque_regions_invalid_names_and_profile_boundaries() {
    let mut p = LspProcess::start().await;
    let cases = [
        (
            "sqlserver",
            "DECLARE @金额 int=1; SELECT N'C:\\'; EXEC app.calculate @first=@金额;".to_owned(),
            false,
        ),
        (
            "sqlserver",
            "DECLARE @计数$# int=1; /* @𐐀 */ EXEC app.calculate @first=@计数$#;".to_owned(),
            false,
        ),
        (
            "sqlserver",
            format!("DECLARE @{} int;", "a".repeat(128)),
            true,
        ),
        ("sqlserver", "DECLARE @𐐀 int;".to_owned(), true),
        ("sqlserver", "DECLARE @½ int;".to_owned(), true),
        ("sqlserver", "DECLARE @ࢠ int;".to_owned(), true),
        ("postgres", "DECLARE @金额 int;".to_owned(), true),
        ("mysql", "DECLARE @金额 int;".to_owned(), true),
    ];
    for (index, (dialect, sql, expected_error)) in cases.iter().enumerate() {
        let uri = format!("oxide://unicode/grammar-{index}.mysql.sql");
        configure(&mut p, &uri, dialect, vec![routine(true, "OUT")]).await;
        p.open(&uri, sql).await;
        assert_eq!(
            !errors(&mut p, &uri).await.is_empty(),
            *expected_error,
            "{sql}"
        );
    }
    p.stop().await;
}
