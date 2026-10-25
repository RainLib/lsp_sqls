use super::execute_values::{complete, configure, errors, offset, routine};
use super::*;

fn position(marked: &str) -> Value {
    let before = &marked[..marked.find("<cursor>").unwrap()];
    let mut line = 0;
    let mut character = 0;
    for ch in before.chars() {
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16();
        }
    }
    json!({"line":line,"character":character})
}
async fn change(p: &mut LspProcess, uri: &str, sql: &str, version: u32) {
    p.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":sql}]}),
    )
    .await;
    p.until(|message| {
        message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["uri"] == uri
            && message["params"]["version"] == version
    })
    .await;
}

#[tokio::test]
async fn native_tsql_modules_accept_utf16_parameter_edits_in_exec_queries_and_functions() {
    let cases=[
        ("sqlserver","/*😀*/ CREATE PROC app.p @input int AS BEGIN EXEC app.calculate @first=@in<cursor> END;","@input","int"),
        ("mssql","CREATE OR ALTER PROCEDURE app.p @input decimal(18,2)=1.25 AS BEGIN\r\n/*😀*/ SELECT CASE WHEN 1=1 THEN @in<cursor> ELSE 0 END; END;","@input","decimal(18,2)"),
        ("tsql","ALTER PROCEDURE app.p @input int AS BEGIN WITH c AS (SELECT 1 AS id) SELECT c.id FROM c WHERE c.id=@in<cursor>put; END;","@input","int"),
        ("sqlserver","CREATE FUNCTION app.f(@input int=1) RETURNS int AS BEGIN RETURN COALESCE(@in<cursor>,0); END;","@input","int"),
        ("mssql","CREATE OR ALTER FUNCTION app.f(@input int) RETURNS TABLE AS RETURN (WITH c AS (SELECT @in<cursor> AS id) SELECT c.id FROM c);","@input","int"),
        ("tsql","/*😀*/ CREATE PROC [app].[例程] @输入$ nvarchar(max)=N'a,b', @输出# int OUTPUT AS BEGIN\r\n/*😀*/ SELECT COALESCE(@输<cursor>入$,N''); END;","@输入$","nvarchar(max)"),
    ];
    let mut p = LspProcess::start().await;
    for (index, (dialect, marked, label, ty)) in cases.iter().enumerate() {
        let uri = format!("oxide://module/accept-{index}.mysql.sql");
        configure(&mut p, &uri, dialect, vec![routine(true, "OUT")]).await;
        let sql = marked.replace("<cursor>", "");
        p.open(&uri, &sql).await;
        assert!(errors(&mut p, &uri).await.is_empty(), "{sql}");
        let result = complete(&mut p, &uri, position(marked)).await;
        let item = result
            .iter()
            .find(|item| item["label"] == *label)
            .unwrap_or_else(|| panic!("{sql}: {result:?}"));
        assert_eq!(item["kind"], 6);
        assert_eq!(item["insertTextFormat"], 1);
        assert!(item["detail"].as_str().unwrap().contains(ty));
        let edit = &item["textEdit"];
        let mut accepted = sql.clone();
        accepted.replace_range(
            offset(&sql, &edit["range"]["start"])..offset(&sql, &edit["range"]["end"]),
            edit["newText"].as_str().unwrap(),
        );
        let expected = marked
            .replace("@in<cursor>put", "@input")
            .replace("@in<cursor>", "@input")
            .replace("<cursor>", "");
        assert_eq!(accepted, expected);
        change(&mut p, &uri, &accepted, 2).await;
        assert!(errors(&mut p, &uri).await.is_empty(), "{accepted}");
    }
    p.stop().await;
}

#[tokio::test]
async fn native_tsql_modules_large_lists_requery_and_live_metadata_profile_refresh() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://module/large.mysql.sql";
    let parameters = (0..2100)
        .map(|n| format!("@arg{n:04} int"))
        .collect::<Vec<_>>()
        .join(",");
    let marked = format!("CREATE PROC app.p {parameters} AS BEGIN SELECT @arg<cursor>; END;");
    let sql = marked.replace("<cursor>", "");
    configure(&mut p, uri, "sqlserver", vec![routine(true, "OUT")]).await;
    p.open(uri, &sql).await;
    assert!(errors(&mut p, uri).await.is_empty());
    let list = p
        .request(
            "textDocument/completion",
            json!({"textDocument":{"uri":uri},"position":position(&marked)}),
        )
        .await;
    assert_eq!(list["isIncomplete"], true);
    assert_eq!(list["items"].as_array().unwrap().len(), 100);
    let marked = marked.replace("@arg<cursor>", "@arg2099<cursor>");
    let sql = marked.replace("<cursor>", "");
    change(&mut p, uri, &sql, 2).await;
    let list = p
        .request(
            "textDocument/completion",
            json!({"textDocument":{"uri":uri},"position":position(&marked)}),
        )
        .await;
    let items = list.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["label"], "@arg2099");
    let uri = "oxide://module/refresh.mysql.sql";
    let marked="CREATE PROC app.p @pickInteger int,@pickNumber decimal(18,2) AS BEGIN EXEC app.calculate @first=@pick<cursor>; END;";
    p.open(uri, &marked.replace("<cursor>", "")).await;
    for (dialect, ty, expected) in [
        ("sqlserver", "int", Some("@pickInteger")),
        ("sqlserver", "decimal(18,2)", Some("@pickNumber")),
        ("mysql", "int", None),
        ("tsql", "int", Some("@pickInteger")),
    ] {
        let mut target = routine(true, "OUT");
        target["parameters"][0]["data_type"] = json!(ty);
        configure(&mut p, uri, dialect, vec![target]).await;
        let items = complete(&mut p, uri, position(marked)).await;
        if let Some(label) = expected {
            assert_eq!(items[0]["label"], label);
            assert!(errors(&mut p, uri).await.is_empty());
        } else {
            assert!(!items.iter().any(|item| item["label"] == "@pickInteger"));
            assert!(!errors(&mut p, uri).await.is_empty());
        }
    }
    p.stop().await;
}

#[tokio::test]
async fn native_tsql_modules_go_noise_invalid_scope_and_readonly_keep_diagnostics() {
    let mut p = LspProcess::start().await;
    let cases=[
        ("CREATE PROC app.p @sharedSecret int AS BEGIN SELECT @sharedSecret; END\r\nGO 2 -- batch\r\nCREATE PROC app.q @sharedVisible int AS BEGIN EXEC app.calculate @first=@sh<cursor>; END;",false,Some("@sharedVisible")),
        ("CREATE FUNCTION app.f(@sharedSecret int) RETURNS int AS BEGIN RETURN @sharedSecret; END\nGO\nDECLARE @sharedVisible int; SELECT @sh<cursor>;",false,Some("@sharedVisible")),
        ("CREATE PROC app.p @sharedVisible int AS BEGIN SELECT N'CREATE PROC app.fake @sharedSecret int AS'; SELECT @sh<cursor>; END;",false,Some("@sharedVisible")),
        ("CREATE PROC app.p @sharedSecret app.InputTable READONLY AS BEGIN SELECT @sh<cursor>; END;",false,None),
        ("CREATE PROC app.p plain int AS BEGIN SELECT @sh<cursor>; END;",true,None),
        ("CREATE FUNCTION app.f(@sharedSecret int OUTPUT) RETURNS int AS BEGIN RETURN 1; END; SELECT @sh<cursor>;",true,None),
        ("SELECT 1; CREATE PROC app.p @sharedSecret int AS BEGIN SELECT @sh<cursor>; END;",true,None),
    ];
    for (index, (marked, invalid, expected)) in cases.iter().enumerate() {
        let uri = format!("oxide://module/scope-{index}.mysql.sql");
        configure(&mut p, &uri, "sqlserver", vec![routine(true, "OUT")]).await;
        let sql = marked.replace("<cursor>", "");
        p.open(&uri, &sql).await;
        assert_eq!(!errors(&mut p, &uri).await.is_empty(), *invalid, "{sql}");
        let items = complete(&mut p, &uri, position(marked)).await;
        assert!(
            !items.iter().any(|item| item["label"] == "@sharedSecret"),
            "{sql}: {items:?}"
        );
        if let Some(label) = expected {
            assert!(
                items.iter().any(|item| item["label"] == *label),
                "{sql}: {items:?}"
            );
        }
    }
    p.stop().await;
}
