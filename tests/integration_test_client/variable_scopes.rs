use super::execute_values::{complete, configure, errors, offset, routine};
use super::*;

fn position(marked: &str) -> Value {
    serde_json::to_value(sql_lsp::position::lsp_position_at_end(
        &marked[..marked.find("<cursor>").unwrap()],
    ))
    .unwrap()
}
async fn change(p: &mut LspProcess, uri: &str, marked: &str, version: u32) {
    p.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":marked.replace("<cursor>","")}]})).await;
    p.until(|m| {
        m["method"] == "textDocument/publishDiagnostics"
            && m["params"]["uri"] == uri
            && m["params"]["version"] == version
    })
    .await;
}
async fn typed(p: &mut LspProcess, uri: &str, marked: &str, ty: &str) {
    let list = complete(p, uri, position(marked)).await;
    assert!(
        list.iter()
            .any(|i| i["label"] == "value" && i["detail"] == format!("Column: value ({ty})")),
        "{marked}: {list:?}"
    );
    let hover = p
        .request(
            "textDocument/hover",
            json!({"textDocument":{"uri":uri},"position":position(marked)}),
        )
        .await;
    assert!(
        serde_json::to_string(&hover).unwrap().contains(ty),
        "{hover:?}"
    );
}

#[tokio::test]
async fn native_variable_scopes_expired_names_keep_errors_and_nested_live_names_accept_edits() {
    let mut p = LspProcess::start().await;
    let negatives=[
        "CREATE FUNCTION app.f(@secret bigint) RETURNS bigint AS BEGIN RETURN @secret; END; SELECT @se<cursor>;",
        "CREATE FUNCTION app.f(@secret bigint) RETURNS bigint AS BEGIN RETURN @secret; END; EXEC app.calculate @first=@se<cursor>;",
        "CREATE FUNCTION app.f() RETURNS bigint AS BEGIN DECLARE @inside bigint; RETURN @inside; END; SELECT @in<cursor>;",
        "CREATE FUNCTION app.f(@secret bigint) RETURNS TABLE AS RETURN(SELECT @secret AS id); SELECT @se<cursor>;",
        "CREATE FUNCTION app.f(@secret bigint) RETURNS TABLE AS RETURN SELECT @secret AS id; SELECT @se<cursor>;",
        "DECLARE @outer bigint; CREATE PROC app.p AS BEGIN SELECT @ou<cursor>; END;",
    ];
    for (n, marked) in negatives.iter().enumerate() {
        let uri = format!("oxide://variable-scopes/expired-{n}.mysql.sql");
        configure(&mut p, &uri, "sqlserver", vec![routine(true, "OUT")]).await;
        p.open(&uri, &marked.replace("<cursor>", "")).await;
        assert!(!errors(&mut p, &uri).await.is_empty(), "{marked}");
        let list = complete(&mut p, &uri, position(marked)).await;
        assert!(
            !list.iter().any(|i| ["@secret", "@inside", "@outer"]
                .iter()
                .any(|name| i["label"] == *name)),
            "{marked}: {list:?}"
        );
    }
    let positives=[
        ("mssql","/*😀*/ CREATE FUNCTION app.f(@input bigint) RETURNS bigint AS BEGIN BEGIN DECLARE @local bigint=CASE WHEN 1=1 THEN @input ELSE 0 END; END;\r\nRETURN @in<cursor>; END;","@input"),
        ("tsql","CREATE PROC app.p @input bigint AS BEGIN SELECT @input; END; SELECT @in<cursor>;","@input"),
        ("sqlserver","CREATE PROC app.p AS BEGIN IF 0=1 BEGIN DECLARE @local bigint; END; SELECT @lo<cursor>; END;","@local"),
    ];
    for (n, (dialect, marked, name)) in positives.iter().enumerate() {
        let uri = format!("oxide://variable-scopes/live-{n}.mysql.sql");
        configure(&mut p, &uri, dialect, vec![]).await;
        let sql = marked.replace("<cursor>", "");
        p.open(&uri, &sql).await;
        assert!(errors(&mut p, &uri).await.is_empty(), "{sql}");
        let list = complete(&mut p, &uri, position(marked)).await;
        let item = list
            .iter()
            .find(|i| i["label"] == *name)
            .unwrap_or_else(|| panic!("{marked}: {list:?}"));
        assert_eq!(item["kind"], 6);
        assert!(item["detail"].as_str().unwrap().contains("bigint"));
        let edit = &item["textEdit"];
        let mut accepted = sql.clone();
        accepted.replace_range(
            offset(&sql, &edit["range"]["start"])..offset(&sql, &edit["range"]["end"]),
            edit["newText"].as_str().unwrap(),
        );
        assert_eq!(
            accepted,
            marked
                .replace("@in<cursor>", "@input")
                .replace("@lo<cursor>", "@local")
        );
        change(&mut p, &uri, &accepted, 2).await;
        assert!(errors(&mut p, &uri).await.is_empty(), "{accepted}");
    }
    p.stop().await;
}

#[tokio::test]
async fn native_variable_scopes_document_boundary_and_profile_edits_refresh_types() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://variable-scopes/refresh.mysql.sql";
    configure(&mut p, uri, "sqlserver", vec![]).await;
    let closed="CREATE FUNCTION app.f(@secret bigint) RETURNS bigint AS BEGIN RETURN @secret; END; WITH p AS(SELECT @secret AS value) SELECT p.va<cursor>lue FROM p;";
    p.open(uri, &closed.replace("<cursor>", "")).await;
    typed(&mut p, uri, closed, "unknown").await;
    assert!(!errors(&mut p, uri).await.is_empty());
    let inside="CREATE FUNCTION app.f(@secret bigint) RETURNS TABLE AS RETURN(WITH p AS(SELECT @secret AS value) SELECT p.va<cursor>lue FROM p);";
    change(&mut p, uri, inside, 2).await;
    typed(&mut p, uri, inside, "bigint").await;
    assert!(errors(&mut p, uri).await.is_empty());
    change(&mut p, uri, closed, 3).await;
    typed(&mut p, uri, closed, "unknown").await;
    for dialect in ["mysql", "tsql"] {
        configure(&mut p, uri, dialect, vec![]).await;
        typed(&mut p, uri, closed, "unknown").await;
    }
    let batch="CREATE FUNCTION app.f(@secret bigint) RETURNS bigint AS BEGIN RETURN @secret; END;\r\nGO 2 -- batch\r\nDECLARE @secret int; WITH p AS(SELECT @secret AS value) SELECT p.va<cursor>lue FROM p;";
    change(&mut p, uri, batch, 4).await;
    typed(&mut p, uri, batch, "int").await;
    assert!(errors(&mut p, uri).await.is_empty());
    p.stop().await;
}

#[tokio::test]
async fn native_variable_scopes_large_functions_budget_and_opaque_prefixes_stay_bounded() {
    let mut p = LspProcess::start().await;
    let parameters = (0..2100)
        .map(|n| format!("@arg{n:04} bigint"))
        .collect::<Vec<_>>()
        .join(",");
    let marked = format!(
        "CREATE FUNCTION app.f({parameters}) RETURNS bigint AS BEGIN RETURN @arg2099<cursor>; END;"
    );
    let uri = "oxide://variable-scopes/large.mysql.sql";
    configure(&mut p, uri, "sqlserver", vec![]).await;
    eprintln!("opening scoped fixture: {uri}");
    p.open(uri, &marked.replace("<cursor>", "")).await;
    assert!(errors(&mut p, uri).await.is_empty());
    let list = complete(&mut p, uri, position(&marked)).await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["label"], "@arg2099");
    let tail = format!(
        "{} SELECT @arg2099<cursor>;",
        marked.replace("<cursor>", "")
    );
    eprintln!("expired tail: {uri}");
    change(&mut p, uri, &tail, 2).await;
    assert!(!complete(&mut p, uri, position(&tail))
        .await
        .iter()
        .any(|i| i["label"] == "@arg2099"));
    assert!(!errors(&mut p, uri).await.is_empty());
    let prefix = (0..4097)
        .map(|n| format!("DECLARE @outer{n:04} int; "))
        .collect::<String>();
    let marked=format!("{prefix}CREATE FUNCTION app.f(@input bigint) RETURNS bigint AS BEGIN RETURN @outer0001<cursor>; END;");
    let uri = "oxide://variable-scopes/budget.mysql.sql";
    configure(&mut p, uri, "mssql", vec![]).await;
    eprintln!("opening scoped fixture: {uri}");
    p.open(uri, &marked.replace("<cursor>", "")).await;
    let list = complete(&mut p, uri, position(&marked)).await;
    assert!(list.is_empty(), "{list:?}");
    assert!(!errors(&mut p, uri).await.is_empty());
    let marked="CREATE PROC app.p @input bigint AS BEGIN SELECT N'CREATE FUNCTION app.fake(@secret int) RETURNS int AS BEGIN END'; /*ALTER PROC app.fake*/ SELECT @in<cursor>; END;";
    let uri = "oxide://variable-scopes/noise.mysql.sql";
    configure(&mut p, uri, "tsql", vec![]).await;
    eprintln!("opening scoped fixture: {uri}");
    p.open(uri, &marked.replace("<cursor>", "")).await;
    let list = complete(&mut p, uri, position(marked)).await;
    assert!(list.iter().any(|i| i["label"] == "@input"));
    assert!(!list.iter().any(|i| i["label"] == "@secret"));
    assert!(errors(&mut p, uri).await.is_empty());
    p.stop().await;
}
