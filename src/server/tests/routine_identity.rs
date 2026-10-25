use super::*;
use tower_lsp::{LanguageServer, LspService};

fn function(name: &str, parameter: &str) -> Function {
    Function {
        name: name.into(),
        parameters: vec![FunctionParameter {
            mode: None,
            name: parameter.into(),
            data_type: "integer".into(),
            optional: false,
        }],
        return_type: "integer".into(),
        routine_type: None,
        description: None,
    }
}

fn schema(database: &str, catalog: Option<&str>, functions: Vec<Function>) -> Schema {
    Schema {
        id: SchemaId::new(),
        server: None,
        namespace: None,
        catalog: catalog.map(str::to_string),
        database: database.into(),
        functions,
        tables: vec![],
        server_version: None,
        source_uri: None,
    }
}

async fn signatures(
    dialect: &str,
    marked: &str,
    current: Schema,
    others: Vec<Schema>,
) -> Vec<String> {
    let (service, _socket) = LspService::new(crate::server::SqlLspServer::new);
    let server = service.inner();
    let uri = Url::parse("oxide://routine/identity.mysql.sql").unwrap();
    let id = server.schema_manager.register(current);
    for schema in others {
        server.schema_manager.register(schema);
    }
    server.configured_file_schemas.insert(uri.to_string(), id);
    server
        .configured_file_dialects
        .insert(uri.to_string(), dialect.into());
    let offset = marked.find("<cursor>").unwrap();
    server
        .document_manager
        .update(uri.to_string(), marked.replace("<cursor>", ""), 1);
    server.signature_help(serde_json::from_value(serde_json::json!({"textDocument":{"uri":uri},"position":lsp_position_at_end(&marked[..offset])})).unwrap()).await.unwrap()
        .map(|help| help.signatures.into_iter().map(|signature| signature.label).collect()).unwrap_or_default()
}

#[tokio::test]
async fn signature_identity_rejects_wrong_namespace_and_qualified_builtin_fallback() {
    for marked in [
        "SELECT other.calc(<cursor>1)",
        "SELECT other.CONCAT(<cursor>'x')",
        "SELECT app.CONCAT(<cursor>'x')",
    ] {
        let result = signatures(
            "postgres",
            marked,
            schema("app", Some("db"), vec![function("calc", "local")]),
            vec![],
        )
        .await;
        assert!(result.is_empty(), "{marked}: {result:?}");
    }
}

#[tokio::test]
async fn signature_identity_preserves_postgres_quoted_case_and_literal_dots() {
    for (marked, parameter) in [
        ("SELECT CALC(<cursor>1)", "lowercase"),
        ("SELECT \"Calc\"(<cursor>1)", "mixedcase"),
        ("SELECT app.\"Calc\"(<cursor>1)", "mixedcase"),
        ("SELECT \"a.b\"(<cursor>1)", "literal_dot"),
        ("SELECT \"a\"\"b\"(<cursor>1)", "escaped_quote"),
    ] {
        let result = signatures(
            "postgres",
            marked,
            schema(
                "app",
                Some("db"),
                vec![
                    function("calc", "lowercase"),
                    function("Calc", "mixedcase"),
                    function("a.b", "literal_dot"),
                    function("a\"b", "escaped_quote"),
                ],
            ),
            vec![],
        )
        .await;
        assert_eq!(result.len(), 1, "{marked}: {result:?}");
        assert!(result[0].contains(parameter), "{marked}: {result:?}");
    }
}

#[tokio::test]
async fn signature_identity_keeps_qualified_paths_across_comments_and_whitespace() {
    for marked in [
        "SELECT app /* 😀 */ . \"Calc\" /* c */ (<cursor>1)",
        "SELECT \"app\"\r\n.\"Calc\"(<cursor>1)",
        "SELECT db.app.\"Calc\"(<cursor>1)",
    ] {
        let result = signatures(
            "postgres",
            marked,
            schema("app", Some("db"), vec![function("Calc", "exact")]),
            vec![],
        )
        .await;
        assert_eq!(result, vec!["Calc(exact integer)"], "{marked}");
    }
}

#[tokio::test]
async fn signature_identity_selects_registered_namespace_within_current_catalog() {
    let result = signatures(
        "postgres",
        "SELECT analytics.calc(<cursor>1)",
        schema("app", Some("db"), vec![function("calc", "local")]),
        vec![
            schema("analytics", Some("db"), vec![function("calc", "remote")]),
            schema(
                "analytics",
                Some("unrelated"),
                vec![function("calc", "secret")],
            ),
        ],
    )
    .await;
    assert_eq!(result, vec!["calc(remote integer)"]);
    for duplicate in [false, true] {
        let mut others = vec![schema(
            "analytics",
            Some("unrelated"),
            vec![function("calc", "secret")],
        )];
        if duplicate {
            for _ in 0..2 {
                others.push(schema(
                    "analytics",
                    Some("db"),
                    vec![function("calc", "ambiguous")],
                ));
            }
        }
        let result = signatures(
            "postgres",
            "SELECT analytics.calc(<cursor>1)",
            schema("app", Some("db"), vec![function("calc", "local")]),
            others,
        )
        .await;
        assert!(result.is_empty(), "{duplicate}: {result:?}");
    }
}

#[tokio::test]
async fn signature_identity_keeps_builtin_quote_and_namespace_rules() {
    for (marked, expected) in [
        ("SELECT pg_catalog.lower(<cursor>'X')", true),
        ("SELECT \"lower\"(<cursor>'X')", true),
        ("SELECT \"LOWER\"(<cursor>'X')", false),
        ("SELECT pg_catalog.\"LOWER\"(<cursor>'X')", false),
        ("SELECT COALESCE(<cursor>NULL,1)", true),
        ("SELECT pg_catalog.coalesce(<cursor>NULL,1)", false),
        ("SELECT \"coalesce\"(<cursor>NULL,1)", false),
    ] {
        let result = signatures(
            "postgres",
            marked,
            schema("app", Some("db"), vec![]),
            vec![],
        )
        .await;
        assert_eq!(!result.is_empty(), expected, "{marked}: {result:?}");
    }
    for (marked, is_udf) in [
        ("SELECT COALESCE(<cursor>NULL,1)", false),
        ("SELECT \"coalesce\"(<cursor>1)", true),
        ("SELECT app.coalesce(<cursor>1)", true),
    ] {
        let result = signatures(
            "postgres",
            marked,
            schema("app", Some("db"), vec![function("coalesce", "udf")]),
            vec![],
        )
        .await;
        assert_eq!(result.len(), 1, "{marked}: {result:?}");
        assert_eq!(result[0].contains("udf"), is_udf, "{marked}: {result:?}");
    }
}

#[tokio::test]
async fn signature_identity_respects_vendor_quoted_names_and_live_precedence() {
    for (dialect, marked, name) in [
        (
            "sqlserver",
            "SELECT [db].[app].[ca]]lc](<cursor>1)",
            "ca]lc",
        ),
        ("mysql", "SELECT `app`.`ca``lc`(<cursor>1)", "ca`lc"),
        ("clickhouse", "SELECT \"app\".\"计算\"(<cursor>1)", "计算"),
    ] {
        let result = signatures(
            dialect,
            marked,
            schema("app", Some("db"), vec![function(name, "value")]),
            vec![],
        )
        .await;
        assert_eq!(result, vec![format!("{name}(value integer)")], "{dialect}");
    }
    let result = signatures(
        "postgres",
        "SELECT app.lower(<cursor>1)",
        schema("app", Some("db"), vec![function("lower", "udf")]),
        vec![],
    )
    .await;
    assert_eq!(result, vec!["lower(udf integer)"]);
}

#[tokio::test]
async fn routine_inlay_hints_use_full_reference_and_comment_safe_argument_offsets() {
    let (service, _socket) = LspService::new(crate::server::SqlLspServer::new);
    let server = service.inner();
    let uri = Url::parse("oxide://routine/hints.sql").unwrap();
    let id = server.schema_manager.register(schema(
        "app",
        Some("db"),
        vec![function("calc", "expected"), function("Calc", "quoted")],
    ));
    server.configured_file_schemas.insert(uri.to_string(), id);
    server
        .configured_file_dialects
        .insert(uri.to_string(), "postgres".into());
    for (sql, label) in [
        ("SELECT app.calc(/* 😀 , ) */ 1)", Some("expected:")),
        ("SELECT other.calc(1)", None),
        ("SELECT app.\"Calc\"(ARRAY[1,2])", Some("quoted:")),
        ("SELECT 'calc(1)'", None),
        ("SELECT $q$ calc(1) $q$", None),
    ] {
        server
            .document_manager
            .update(uri.to_string(), sql.into(), 1);
        let hints=server.inlay_hint(serde_json::from_value(serde_json::json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":lsp_position_at_end(sql)}})).unwrap()).await.unwrap().unwrap();
        let labels = hints
            .iter()
            .map(|hint| match &hint.label {
                InlayHintLabel::String(label) => label.as_str(),
                _ => "parts",
            })
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            label.into_iter().collect::<Vec<_>>(),
            "{sql}: {hints:?}"
        );
        if let Some(hint) = hints.first() {
            let start = sql
                .rfind(if sql.contains("ARRAY") { "ARRAY" } else { "1" })
                .unwrap();
            assert_eq!(hint.position, lsp_position_at_end(&sql[..start]), "{sql}");
        }
    }
}

#[tokio::test]
async fn signature_identity_does_not_guess_missing_catalog_or_overlong_paths() {
    for marked in [
        "SELECT analytics.calc(<cursor>1)",
        "SELECT server.db.app.calc(<cursor>1)",
        "SELECT wrong.app.calc(<cursor>1)",
    ] {
        let result = signatures(
            "postgres",
            marked,
            schema("app", None, vec![function("calc", "local")]),
            vec![schema(
                "analytics",
                None,
                vec![function("calc", "unproven")],
            )],
        )
        .await;
        assert!(result.is_empty(), "{marked}: {result:?}");
    }
}

#[test]
fn routine_identity_inlay_arguments_keep_noise_literals_and_vendor_boundaries() {
    use super::super::routine_hints::routine_parameter_hints;
    let manager = SchemaManager::new();
    let metadata = schema("app", Some("db"), vec![function("calc", "expected")]);
    for (dialect, sql, expected) in [
        (
            "postgres",
            "SELECT app.calc(/* a /* , ) */ b */ $q$ , ) $q$)",
            true,
        ),
        ("clickhouse", "SELECT app.calc([1,2]) // calc(3)", true),
        ("mysql", "SELECT app.calc('it\\'s,)') # calc(3)", true),
        ("sqlserver", "SELECT [app].[calc]([a,b])", true),
        ("hive", "SELECT app.calc(\"a,b)\")", true),
        ("postgres", "SELECT app.calc(expected => 1)", false),
        ("postgres", "SELECT app.calc(1,)", false),
        ("postgres", "SELECT app.calc(/* comment */)", false),
        ("postgres", "SELECT app.calc(ARRAY[1,2]", false),
    ] {
        let hints = routine_parameter_hints(
            sql,
            range_for_offsets(sql, 0, sql.len()),
            &metadata,
            &manager,
            dialect,
        );
        assert_eq!(
            hints.len(),
            usize::from(expected),
            "{dialect}: {sql}: {hints:?}"
        );
    }
    let ambiguous = schema(
        "app",
        Some("db"),
        vec![function("calc", "left"), function("calc", "right")],
    );
    let sql = "SELECT app.calc(1)";
    assert!(routine_parameter_hints(
        sql,
        range_for_offsets(sql, 0, sql.len()),
        &ambiguous,
        &manager,
        "postgres"
    )
    .is_empty());
}

#[test]
fn routine_identity_inlay_ranges_keep_utf16_and_bounded_argument_work() {
    use super::super::routine_hints::routine_parameter_hints;
    let manager = SchemaManager::new();
    let mut routine = function("calc", "first");
    routine.parameters.push(FunctionParameter {
        mode: None,
        name: "second".into(),
        data_type: "integer".into(),
        optional: false,
    });
    let metadata = schema("app", Some("db"), vec![routine]);
    let sql = "/* 😀 */\r\nSELECT app.calc(/* 😀 ,) */ 1, ARRAY[2,3]);";
    let array = sql.find("ARRAY").unwrap();
    let hints = routine_parameter_hints(
        sql,
        range_for_offsets(sql, array, sql.len()),
        &metadata,
        &manager,
        "postgres",
    );
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].position, lsp_position_at_end(&sql[..array]));
    let oversized = format!("SELECT app.calc(1, {}2)", " ".repeat(70_000));
    assert!(routine_parameter_hints(
        &oversized,
        range_for_offsets(&oversized, 0, oversized.len()),
        &metadata,
        &manager,
        "postgres"
    )
    .is_empty());
    let deep = format!("SELECT {}1{}", "(".repeat(20_000), ")".repeat(20_000));
    let started = std::time::Instant::now();
    assert!(routine_parameter_hints(
        &deep,
        range_for_offsets(&deep, 0, deep.len()),
        &metadata,
        &manager,
        "postgres"
    )
    .is_empty());
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}
