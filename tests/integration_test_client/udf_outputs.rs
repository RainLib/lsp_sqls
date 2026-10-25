use super::*;

fn function(name: &str, input: &str, result: &str) -> Value {
    json!({"name":name,"routine_type":"function","parameters":[{"name":"value","data_type":input,"optional":false}],"return_type":result})
}
fn metadata(id: &str, database: &str, catalog: &str, functions: Vec<Value>) -> Value {
    json!({"id":id,"database":database,"catalog":catalog,"tables":[{"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"name","data_type":"text","nullable":false}]}],"functions":functions})
}
async fn configure(process: &mut LspProcess, uri: &str, dialect: &str, schemas: Vec<Value>) {
    process.notify("workspace/didChangeConfiguration",json!({"settings":{"schemas":schemas,"fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):dialect}}})).await;
}
async fn check(process: &mut LspProcess, uri: &str, marked: &str, ty: &str, nullable: bool) {
    let offset = marked.find("<cursor>").unwrap();
    let sql = marked.replace("<cursor>", "");
    let params = json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..offset])});
    let result = process
        .request("textDocument/completion", params.clone())
        .await;
    let items = result
        .as_array()
        .or_else(|| result["items"].as_array())
        .unwrap();
    let item = items
        .iter()
        .find(|item| item["label"] == "value")
        .unwrap_or_else(|| panic!("{sql}: {result}"));
    assert_eq!(
        item["detail"],
        format!("Column: value ({ty})"),
        "{sql}: {item}"
    );
    assert!(
        item["documentation"].to_string().contains(if nullable {
            "Nullable: yes"
        } else {
            "Nullable: no"
        }),
        "{sql}: {item}"
    );
    let start = sql_lsp::position::lsp_position_at_end(&sql[..offset - 2]);
    let end = sql_lsp::position::lsp_position_at_end(&sql[..offset + 3]);
    assert_eq!(item["textEdit"]["range"], json!({"start":start,"end":end}));
    assert_eq!(item["textEdit"]["newText"], "value");
    // The actual returned replacement reconstructs the same complete field.
    let applied = format!(
        "{}{}{}",
        &sql[..offset - 2],
        item["textEdit"]["newText"].as_str().unwrap(),
        &sql[offset + 3..]
    );
    assert_eq!(applied, sql);
    let hover = process.request("textDocument/hover", params).await;
    assert!(hover.to_string().contains(ty), "{sql}: {hover}");
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
async fn native_udf_outputs_feed_completion_hover_through_complex_query_shapes() {
    let mut process = LspProcess::start().await;
    let cases=[
        ("postgres","WITH p AS (SELECT app.calc(1) AS value) SELECT p.va<cursor>lue FROM p","calc","bigint",true),
        ("postgres","SELECT d.va<cursor>lue FROM (SELECT app.calc(1) AS value) d","calc","bigint",true),
        ("postgres","WITH p AS (SELECT (SELECT app.calc(1)) AS value) SELECT p.va<cursor>lue FROM p","calc","bigint",true),
        ("postgres","WITH p AS (SELECT CASE WHEN TRUE THEN app.calc(1) ELSE 2::bigint END AS value) SELECT p.va<cursor>lue FROM p","calc","bigint",true),
        ("postgres","WITH p AS (SELECT COALESCE(app.calc(1),2::bigint) AS value) SELECT p.va<cursor>lue FROM p","calc","bigint",false),
        ("postgres","WITH RECURSIVE p(value) AS (SELECT app.calc(1) UNION ALL SELECT value FROM p WHERE FALSE) SELECT p.va<cursor>lue FROM p","calc","bigint",true),
        ("postgres","WITH p AS (SELECT app.calc(value=>1) AS value) SELECT p.va<cursor>lue FROM p","calc","bigint",true),
        ("postgres","WITH p AS (SELECT \"app\".\"CaLc\"(1) AS value) SELECT p.va<cursor>lue FROM p","CaLc","bigint",true),
        ("postgres","WITH p AS (SELECT calc(NULL) AS value) SELECT p.va<cursor>lue FROM p","calc","unknown",true),
        ("postgres","WITH p AS (SELECT count(1) AS value) SELECT p.va<cursor>lue FROM p","count","bigint",true),
        ("mysql","SELECT d.va<cursor>lue FROM (SELECT `app`.`calc`(1) AS value) d","calc","bigint",true),
        ("sqlserver","WITH p AS (SELECT [app].[calc](1) AS value) SELECT p.va<cursor>lue FROM p","calc","bigint",true),
        ("postgres","SELECT d.va<cursor>lue FROM users u CROSS JOIN LATERAL (SELECT app.calc(u.id) AS value) d","calc","numeric",true),
        ("postgres","WITH p AS (SELECT app.calc(1) AS value UNION ALL SELECT 2::bigint) SELECT p.va<cursor>lue FROM p","calc","bigint",true),
        ("postgres","WITH p AS (SELECT app.fixed(1) AS value) SELECT p.va<cursor>lue FROM p","fixed","bigint",true),
        ("postgres","WITH p AS (SELECT app.fold_values(VARIADIC(ARRAY[1,2]::integer[])) AS value) SELECT p.va<cursor>lue FROM p","fold_values","bigint",true),
    ];
    assert_eq!(cases.len(), 16);
    for (index, (dialect, marked, name, ty, nullable)) in cases.into_iter().enumerate() {
        let uri = format!("oxide://udf/shapes.{index}.mysql.sql");
        configure(
            &mut process,
            &uri,
            dialect,
            vec![metadata(
                "aaaaaaaa-1111-4111-8111-111111111111",
                "app",
                "db",
                if name == "fold_values" {
                    let mut variadic = function(name, "integer[]", "bigint");
                    variadic["parameters"][0]["mode"] = json!("VARIADIC");
                    vec![variadic]
                } else if name == "fixed" {
                    let mut variadic = function(name, "integer[]", "uuid");
                    variadic["parameters"][0]["mode"] = json!("VARIADIC");
                    vec![variadic, function(name, "integer", "bigint")]
                } else if dialect == "mysql" {
                    vec![function(name, "bigint", "bigint")]
                } else if dialect == "sqlserver" {
                    vec![function(name, "int", "bigint")]
                } else {
                    vec![
                        function(name, "text", "uuid"),
                        function(name, "integer", "bigint"),
                        function(name, "bigint", "numeric"),
                    ]
                },
            )],
        )
        .await;
        let marked = format!("/* 😀 */\r\n/* 😀 */ {marked};");
        process.open(&uri, &marked.replace("<cursor>", "")).await;
        check(&mut process, &uri, &marked, ty, nullable).await;
        if name == "fold_values" {
            let sql = marked.replace("<cursor>", "");
            let hints=process.request("textDocument/inlayHint",json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":sql_lsp::position::lsp_position_at_end(&sql)}})).await;
            assert_eq!(hints.as_array().unwrap().len(), 1, "{hints}");
            assert_eq!(hints[0]["tooltip"], "Parameter type: integer[]", "{hints}");
        }
    }
    process.stop().await;
}

#[tokio::test]
async fn native_udf_outputs_refresh_remote_catalog_identity_and_returns_without_sql_edits() {
    let mut process = LspProcess::start().await;
    let uri = "oxide://udf/refresh.mysql.sql";
    let marked =
        "/* 😀 */\r\nWITH p AS (SELECT analytics.calc(1) AS value) SELECT p.va<cursor>lue FROM p;";
    let current = metadata(
        "aaaaaaaa-1111-4111-8111-111111111111",
        "app",
        "db",
        vec![function("calc", "integer", "text")],
    );
    let remote = |ty: &str| {
        metadata(
            "bbbbbbbb-1111-4111-8111-111111111111",
            "analytics",
            "db",
            vec![function("calc", "integer", ty)],
        )
    };
    let rounds = [
        (vec![current.clone(), remote("bigint")], "bigint"),
        (vec![current.clone(), remote("uuid")], "uuid"),
        (
            vec![
                current.clone(),
                remote("uuid"),
                metadata(
                    "cccccccc-1111-4111-8111-111111111111",
                    "analytics",
                    "db",
                    vec![function("calc", "integer", "bigint")],
                ),
            ],
            "unknown",
        ),
        (
            vec![
                current.clone(),
                metadata(
                    "bbbbbbbb-1111-4111-8111-111111111111",
                    "analytics",
                    "foreign",
                    vec![function("calc", "integer", "uuid")],
                ),
            ],
            "unknown",
        ),
        (vec![current.clone()], "unknown"),
        (vec![current, remote("bigint")], "bigint"),
    ];
    assert_eq!(rounds.len(), 6);
    for (index, (schemas, ty)) in rounds.into_iter().enumerate() {
        configure(&mut process, uri, "postgres", schemas).await;
        if index == 0 {
            process.open(uri, &marked.replace("<cursor>", "")).await;
        }
        check(&mut process, uri, marked, ty, true).await;
    }
    process.stop().await;
}

#[tokio::test]
async fn native_udf_outputs_rank_outer_signatures_from_live_remote_returns() {
    let mut process = LspProcess::start().await;
    let uri = "oxide://udf/nested-rank.mysql.sql";
    let sql = "/* 😀 */\r\nSELECT pick(analytics.calc(1));";
    let cursor = sql.rfind(");").unwrap();
    let mut number = function("pick", "bigint", "integer");
    number["parameters"][0]["name"] = json!("number");
    let mut identifier = function("pick", "uuid", "integer");
    identifier["parameters"][0]["name"] = json!("identifier");
    let current = metadata(
        "aaaaaaaa-1111-4111-8111-111111111111",
        "app",
        "db",
        vec![number, identifier],
    );
    let rounds = [
        (Some("bigint"), "pick(number bigint)"),
        (Some("uuid"), "pick(identifier uuid)"),
        (Some("record"), "pick(number bigint)"),
        (None, "pick(number bigint)"),
        (Some("int8"), "pick(number bigint)"),
        (Some("uuid"), "pick(identifier uuid)"),
    ];
    assert_eq!(rounds.len(), 6);
    for (index, (result, wanted)) in rounds.into_iter().enumerate() {
        let mut schemas = vec![current.clone()];
        if let Some(result) = result {
            schemas.push(metadata(
                "bbbbbbbb-1111-4111-8111-111111111111",
                "analytics",
                "db",
                vec![function("calc", "integer", result)],
            ));
        }
        configure(&mut process, uri, "postgres", schemas).await;
        if index == 0 {
            process.open(uri, sql).await;
        }
        let help=process.request("textDocument/signatureHelp",json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..cursor])})).await;
        assert_eq!(
            help["signatures"].as_array().unwrap().len(),
            2,
            "round {index}: {help}"
        );
        assert_eq!(
            help["signatures"][0]["label"], wanted,
            "round {index}: {help}"
        );
        assert_eq!(help["activeParameter"], 0);
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
            "{diagnostics}"
        );
    }
    process.stop().await;
}
