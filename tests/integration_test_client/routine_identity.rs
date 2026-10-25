use super::*;
use tower_lsp::lsp_types::Position;

fn routine(name: &str, parameter: &str) -> Value {
    json!({"name":name,"parameters":[{"name":parameter,"data_type":"integer","optional":false}],"return_type":"integer","description":null})
}

fn metadata(id: &str, database: &str, catalog: &str, functions: Vec<Value>) -> Value {
    json!({"id":id,"database":database,"catalog":catalog,"tables":[],"functions":functions})
}

async fn configure(process: &mut LspProcess, uri: &str, dialect: &str, schemas: Vec<Value>) {
    process.notify("workspace/didChangeConfiguration", json!({"settings":{"schemas":schemas,
        "fileSchemas":{(uri):"cccccccc-cccc-4ccc-8ccc-cccccccccccc"},"fileDialects":{(uri):dialect}}})).await;
}

async fn signature(process: &mut LspProcess, uri: &str, position: Position) -> Value {
    process
        .request(
            "textDocument/signatureHelp",
            json!({"textDocument":{"uri":uri},"position":position}),
        )
        .await
}

#[tokio::test]
async fn native_routine_identity_signatures_keep_namespaces_quotes_and_catalogs() {
    let mut process = LspProcess::start().await;
    let cases = [
        (
            "postgres",
            "SELECT CALC(<cursor>1)",
            Some("calc(local integer)"),
        ),
        (
            "postgres",
            "SELECT \"Calc\"(<cursor>1)",
            Some("Calc(quoted integer)"),
        ),
        ("postgres", "SELECT other.calc(<cursor>1)", None),
        ("postgres", "SELECT app.CONCAT(<cursor>'x')", None),
        (
            "postgres",
            "SELECT pg_catalog.lower(<cursor>'X')",
            Some("LOWER(string)"),
        ),
        ("postgres", "SELECT \"LOWER\"(<cursor>'X')", None),
        (
            "postgres",
            "SELECT \"lower\"(<cursor>'X')",
            Some("LOWER(string)"),
        ),
        (
            "postgres",
            "SELECT \"a.b\"(<cursor>1)",
            Some("a.b(dotted integer)"),
        ),
        (
            "postgres",
            "SELECT app /* 😀 */ . \"Calc\" /* ,) */ (<cursor>1)",
            Some("Calc(quoted integer)"),
        ),
        (
            "postgres",
            "SELECT ARRAY[app.\"Calc\"(<cursor>1)]",
            Some("Calc(quoted integer)"),
        ),
        (
            "postgres",
            "SELECT analytics.calc(<cursor>1)",
            Some("calc(remote integer)"),
        ),
        ("postgres", "SELECT foreign.app.calc(<cursor>1)", None),
        (
            "postgres",
            "SELECT app.lower(<cursor>1)",
            Some("lower(udf integer)"),
        ),
        (
            "postgres",
            "SELECT \"a\"\"b\"(<cursor>1)",
            Some("a\"b(escaped integer)"),
        ),
        (
            "mysql",
            "SELECT `app`.`ca``lc`(<cursor>1)",
            Some("ca`lc(tick integer)"),
        ),
        (
            "sqlserver",
            "SELECT [db].[app].[ca]]lc](<cursor>1)",
            Some("ca]lc(bracket integer)"),
        ),
        (
            "clickhouse",
            "SELECT \"app\".\"计算\"(<cursor>1)",
            Some("计算(unicode integer)"),
        ),
        ("clickhouse", "SELECT app.CALC(<cursor>1)", None),
    ];
    assert_eq!(cases.len(), 18);
    for (index, (dialect, marked, expected)) in cases.into_iter().enumerate() {
        let uri = format!("oxide://routine/qualified.{index}.mysql.sql");
        let mut functions = vec![
            routine("calc", "local"),
            routine("Calc", "quoted"),
            routine("a.b", "dotted"),
            routine("lower", "udf"),
            routine("a\"b", "escaped"),
            routine("ca`lc", "tick"),
            routine("ca]lc", "bracket"),
            routine("计算", "unicode"),
        ];
        // Builtin lookup fixtures omit a conflicting live routine. Separate
        // app.lower coverage verifies the established live-metadata priority.
        if expected == Some("LOWER(string)") {
            functions.retain(|function| function["name"] != "lower");
        }
        configure(
            &mut process,
            &uri,
            dialect,
            vec![
                metadata(
                    "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
                    "app",
                    "db",
                    functions,
                ),
                metadata(
                    "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
                    "analytics",
                    "db",
                    vec![routine("calc", "remote")],
                ),
                metadata(
                    "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee",
                    "analytics",
                    "foreign",
                    vec![routine("calc", "secret")],
                ),
            ],
        )
        .await;
        let marked = format!("/* 😀 */\r\n/* 😀 */ {marked};");
        let offset = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let result = signature(
            &mut process,
            &uri,
            sql_lsp::position::lsp_position_at_end(&sql[..offset]),
        )
        .await;
        if let Some(expected) = expected {
            assert_eq!(
                result["signatures"].as_array().unwrap().len(),
                1,
                "{dialect}: {sql}: {result}"
            );
            assert_eq!(
                result["signatures"][0]["label"], expected,
                "{dialect}: {sql}: {result}"
            );
            assert_eq!(result["activeParameter"], 0);
        } else {
            assert!(result.is_null(), "{dialect}: {sql}: {result}");
        }
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
            "{dialect}: {sql}: {diagnostics}"
        );
    }
    process.stop().await;
}

#[tokio::test]
async fn native_routine_identity_refreshes_signature_and_inlay_without_document_edits() {
    let mut process = LspProcess::start().await;
    let uri = "oxide://routine/refresh.mysql.sql";
    let sql = "/* 😀 */\r\nSELECT analytics.\"Calc\"(/* 😀 ,) */ ARRAY[1,2]);";
    let argument = sql.find("ARRAY").unwrap();
    let position = sql_lsp::position::lsp_position_at_end(&sql[..argument]);
    for (round, expected) in [
        Some("old"),
        Some("updated"),
        None,
        None,
        Some("restored"),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        let mut schemas = vec![metadata(
            "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
            "app",
            "db",
            vec![routine("Calc", "wrong_schema")],
        )];
        if round != 2 {
            schemas.push(metadata(
                "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
                "analytics",
                if round == 3 { "other" } else { "db" },
                vec![routine("Calc", expected.unwrap_or("stale"))],
            ));
        }
        if round == 5 {
            schemas.push(metadata(
                "ffffffff-ffff-4fff-8fff-ffffffffffff",
                "analytics",
                "db",
                vec![routine("Calc", "duplicate")],
            ));
        }
        configure(&mut process, uri, "postgres", schemas).await;
        if round == 0 {
            process.open(uri, sql).await;
        }
        let help = signature(&mut process, uri, position).await;
        let hints=process.request("textDocument/inlayHint",json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":sql_lsp::position::lsp_position_at_end(sql)}})).await;
        if let Some(expected) = expected {
            assert_eq!(
                help["signatures"][0]["label"],
                format!("Calc({expected} integer)"),
                "round {round}: {help}"
            );
            assert_eq!(hints.as_array().unwrap().len(), 1, "round {round}: {hints}");
            assert_eq!(hints[0]["label"], format!("{expected}:"));
            assert_eq!(hints[0]["position"], json!(position));
        } else {
            assert!(help.is_null(), "round {round}: {help}");
            assert!(
                hints.as_array().unwrap().is_empty(),
                "round {round}: {hints}"
            );
        }
    }
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
    process.stop().await;
}
