use super::execute_values::{complete, configure, errors, offset};
use super::*;

fn position(marked: &str) -> Value {
    serde_json::to_value(sql_lsp::position::lsp_position_at_end(
        &marked[..marked.find("<cursor>").unwrap()],
    ))
    .unwrap()
}
async fn change(p: &mut LspProcess, uri: &str, sql: &str, version: u32) {
    p.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":sql}]}),
    )
    .await;
    p.until(|m| {
        m["method"] == "textDocument/publishDiagnostics"
            && m["params"]["uri"] == uri
            && m["params"]["version"] == version
    })
    .await;
}
async fn value(p: &mut LspProcess, uri: &str, marked: &str, ty: &str) -> Value {
    let items = complete(p, uri, position(marked)).await;
    let item = items
        .iter()
        .find(|i| i["label"] == "value")
        .unwrap_or_else(|| panic!("{marked}: {items:?}"));
    assert_eq!(item["detail"], format!("Column: value ({ty})"));
    assert!(serde_json::to_string(&item["documentation"])
        .unwrap()
        .contains("Nullable: yes"));
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
    item.clone()
}
fn functions(result: &str) -> Vec<Value> {
    vec![
        json!({"name":"pick","routine_type":"function","return_type":"nvarchar(max)","parameters":[{"name":"wrong","data_type":"int","optional":false,"mode":"IN"}]}),
        json!({"name":"pick","routine_type":"function","return_type":result,"parameters":[{"name":"wanted","data_type":"bigint","optional":false,"mode":"IN"}]}),
    ]
}

#[tokio::test]
async fn native_variable_types_complex_projection_edits_hover_and_dialects() {
    let cases=[
        ("sqlserver","/*😀*/ DECLARE @金额$ bigint;\r\nWITH p AS(SELECT @金额$ AS value),q AS(SELECT value FROM p) SELECT q.va<cursor>lue FROM q;","bigint"),
        ("mssql","DECLARE @input decimal(18,2); SELECT d.va<cursor>lue FROM (SELECT @input AS value) d;","decimal(18,2)"),
        ("tsql","DECLARE @input datetime2(3); WITH p AS(SELECT (SELECT @input) AS value) SELECT p.va<cursor>lue FROM p;","datetime2(3)"),
        ("sqlserver","DECLARE @a int,@b bigint; WITH p AS(SELECT CASE WHEN 1=1 THEN @a ELSE @b END AS value) SELECT p.va<cursor>lue FROM p;","bigint"),
        ("mssql","CREATE PROC app.p @input nvarchar(max) AS BEGIN WITH p AS(SELECT COALESCE(@input,@input) AS value) SELECT p.va<cursor>lue FROM p; END;","nvarchar(max)"),
        ("tsql","CREATE FUNCTION app.f(@input bigint) RETURNS TABLE AS RETURN (WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p);","bigint"),
    ];
    let mut p = LspProcess::start().await;
    for (index, (dialect, marked, ty)) in cases.iter().enumerate() {
        let uri = format!("oxide://variable-types/edit-{index}.mysql.sql");
        configure(&mut p, &uri, dialect, vec![]).await;
        let sql = marked.replace("<cursor>", "");
        p.open(&uri, &sql).await;
        assert!(errors(&mut p, &uri).await.is_empty(), "{sql}");
        let item = value(&mut p, &uri, marked, ty).await;
        let edit = &item["textEdit"];
        let mut accepted = sql.clone();
        accepted.replace_range(
            offset(&sql, &edit["range"]["start"])..offset(&sql, &edit["range"]["end"]),
            edit["newText"].as_str().unwrap(),
        );
        assert_eq!(accepted, marked.replace("va<cursor>lue", "value"));
        change(&mut p, &uri, &accepted, 2).await;
        assert!(errors(&mut p, &uri).await.is_empty(), "{accepted}");
    }
    p.stop().await;
}

#[tokio::test]
async fn native_variable_types_signature_metadata_document_and_profile_refresh() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://variable-types/refresh.mysql.sql";
    configure(&mut p, uri, "sqlserver", functions("bigint")).await;
    let signature = "DECLARE @input bigint; SELECT app.pick(@input<cursor>);";
    p.open(uri, &signature.replace("<cursor>", "")).await;
    let help = p
        .request(
            "textDocument/signatureHelp",
            json!({"textDocument":{"uri":uri},"position":position(signature)}),
        )
        .await;
    assert!(help["signatures"][0]["label"]
        .as_str()
        .unwrap()
        .contains("wanted"));
    assert_eq!(help["signatures"].as_array().unwrap().len(), 2);
    let mut marked =
        "DECLARE @input int; WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p;"
            .to_owned();
    change(&mut p, uri, &marked.replace("<cursor>", ""), 2).await;
    value(&mut p, uri, &marked, "int").await;
    marked = marked.replace("@input int;", "@input bigint;");
    change(&mut p, uri, &marked.replace("<cursor>", ""), 3).await;
    value(&mut p, uri, &marked, "bigint").await;
    marked = marked.replace("SELECT @input AS value", "SELECT app.pick(@input) AS value");
    change(&mut p, uri, &marked.replace("<cursor>", ""), 4).await;
    value(&mut p, uri, &marked, "bigint").await;
    configure(&mut p, uri, "mssql", functions("uniqueidentifier")).await;
    value(&mut p, uri, &marked, "uniqueidentifier").await;
    configure(&mut p, uri, "mysql", functions("uniqueidentifier")).await;
    value(&mut p, uri, &marked, "unknown").await;
    configure(&mut p, uri, "tsql", functions("uniqueidentifier")).await;
    value(&mut p, uri, &marked, "uniqueidentifier").await;
    assert!(errors(&mut p, uri).await.is_empty());
    p.stop().await;
}

#[tokio::test]
async fn native_variable_types_scope_unknown_and_large_module_projection() {
    let mut p = LspProcess::start().await;
    let cases=[
        "DECLARE @input bigint;\nGO\nWITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p;",
        "WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p; DECLARE @input bigint;",
        "DECLARE @input int; DECLARE @input bigint; WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p;",
        "CREATE PROC app.p @input app.TableType READONLY AS BEGIN WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p; END;",
        "DECLARE @input app.UnknownType; WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p;",
    ];
    for (index, marked) in cases.iter().enumerate() {
        let uri = format!("oxide://variable-types/scope-{index}.mysql.sql");
        configure(&mut p, &uri, "sqlserver", vec![]).await;
        p.open(&uri, &marked.replace("<cursor>", "")).await;
        value(&mut p, &uri, marked, "unknown").await;
    }
    let parameters = (0..2100)
        .map(|n| format!("@arg{n:04} bigint"))
        .collect::<Vec<_>>()
        .join(",");
    let marked=format!("CREATE PROC app.p {parameters} AS BEGIN WITH p AS(SELECT @arg2099 AS value) SELECT p.va<cursor>lue FROM p; END;");
    let uri = "oxide://variable-types/large.mysql.sql";
    configure(&mut p, uri, "sqlserver", vec![]).await;
    p.open(uri, &marked.replace("<cursor>", "")).await;
    assert!(errors(&mut p, uri).await.is_empty());
    value(&mut p, uri, &marked, "bigint").await;
    p.stop().await;
}
