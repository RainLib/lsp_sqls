use super::*;
use tower_lsp::{LanguageServer, LspService};

#[tokio::test]
async fn postgres_catalog_types_rank_the_same_builtin_identity_without_custom_name_guessing() {
    first(
        "postgres",
        "SELECT app.pick(CAST(1 AS pg_catalog.int4)<cursor>)",
        "text",
        "integer",
    )
    .await;
    first(
        "postgres",
        "SELECT app.pick(CAST(TRUE AS \"pg_catalog\".\"bool\")<cursor>)",
        "integer",
        "bool",
    )
    .await;
    for ty in ["public.int4", "\"PG_CATALOG\".int4", "pg_catalog.\"INT4\""] {
        assert_eq!(
            super::super::routine_overloads::canonical_type(ty, "postgres"),
            None,
            "{ty}"
        );
    }
}

fn routine(route: &str, types: &[&str]) -> Function {
    Function {
        name: "pick".into(),
        routine_type: None,
        return_type: "integer".into(),
        description: None,
        parameters: types
            .iter()
            .enumerate()
            .map(|(index, ty)| FunctionParameter {
                mode: None,
                name: format!("{route}_{index}"),
                data_type: (*ty).into(),
                optional: false,
            })
            .collect(),
    }
}

async fn ranked(dialect: &str, marked: &str, functions: Vec<Function>) -> Vec<String> {
    let (service, _socket) = LspService::new(crate::server::SqlLspServer::new);
    let server = service.inner();
    let uri = Url::parse("oxide://routine/overload.mysql.sql").unwrap();
    let mut schema = semantic_test_schema();
    schema.catalog = Some("db".into());
    schema.database = "app".into();
    schema.functions = functions;
    let id = server.schema_manager.register(schema);
    server.configured_file_schemas.insert(uri.to_string(), id);
    server
        .configured_file_dialects
        .insert(uri.to_string(), dialect.into());
    let offset = marked.find("<cursor>").unwrap();
    server
        .document_manager
        .update(uri.to_string(), marked.replace("<cursor>", ""), 1);
    let help=server.signature_help(serde_json::from_value(serde_json::json!({"textDocument":{"uri":uri},"position":lsp_position_at_end(&marked[..offset])})).unwrap()).await.unwrap().expect("signature");
    help.signatures
        .into_iter()
        .map(|signature| signature.label)
        .collect()
}

async fn first(dialect: &str, sql: &str, wrong: &str, wanted: &str) {
    let result = ranked(
        dialect,
        sql,
        vec![routine("wrong", &[wrong]), routine("wanted", &[wanted])],
    )
    .await;
    assert!(result[0].contains("wanted"), "{dialect}: {sql}: {result:?}");
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn routine_overloads_prefer_known_literals_casts_and_expression_results() {
    for (sql, wrong, wanted) in [
        ("SELECT app.pick(1<cursor>)", "text", "integer"),
        ("SELECT app.pick(2147483648<cursor>)", "integer", "int8"),
        ("SELECT app.pick(TRUE<cursor>)", "integer", "bool"),
        ("SELECT app.pick(1.25<cursor>)", "integer", "numeric"),
        (
            "SELECT app.pick(CAST('x' AS text)<cursor>)",
            "integer",
            "text",
        ),
        (
            "SELECT app.pick((COALESCE(NULL,1)+2)<cursor>)",
            "text",
            "int4",
        ),
        (
            "SELECT app.pick(CASE WHEN 1=1 THEN 2 ELSE 3 END<cursor>)",
            "text",
            "integer",
        ),
        (
            "SELECT app.pick(ARRAY[1,2]::integer[]<cursor>)",
            "integer",
            "int4[]",
        ),
    ] {
        first("postgres", sql, wrong, wanted).await;
    }
}

#[tokio::test]
async fn routine_overloads_use_query_fields_ctes_and_scalar_outputs() {
    for sql in [
        "SELECT app.pick(u.id<cursor>) FROM users u",
        "WITH p AS (SELECT u.id AS value FROM users u) SELECT app.pick(p.value<cursor>) FROM p",
        "SELECT app.pick(p.value<cursor>) FROM (SELECT id AS value FROM users) p",
        "SELECT app.pick((SELECT MAX(u.id) FROM users u)<cursor>)",
        "SELECT 1 FROM users u WHERE EXISTS (SELECT app.pick(u.id<cursor>))",
    ] {
        first("postgres", sql, "text", "bigint").await;
    }
}

#[tokio::test]
async fn routine_overloads_use_completed_arguments_and_keep_unknowns_stable() {
    let result = ranked(
        "postgres",
        "SELECT app.pick(/* 😀 ,) */ 1, <cursor>NULL)",
        vec![
            routine("wrong", &["text", "integer"]),
            routine("wanted", &["integer", "text"]),
        ],
    )
    .await;
    assert!(result[0].contains("wanted"), "{result:?}");
    for marked in [
        "SELECT app.pick('not typed'<cursor>)",
        "SELECT app.pick(NULL<cursor>)",
        "SELECT app.pick(unknown_func(1)<cursor>)",
        "SELECT app.pick(CAST(1 AS custom_domain)<cursor>)",
        "SELECT app.pick(1<cursor>+2.5)",
    ] {
        let result = ranked(
            "postgres",
            marked,
            vec![
                routine("original", &["text"]),
                routine("other", &["integer"]),
            ],
        )
        .await;
        assert!(result[0].contains("original"), "{marked}: {result:?}");
    }
}

#[tokio::test]
async fn routine_overloads_keep_vendor_type_aliases_separate() {
    for (dialect, sql, wrong, wanted) in [
        ("mysql", "SELECT app.pick(1<cursor>)", "text", "bigint"),
        ("sqlserver", "SELECT app.pick(1<cursor>)", "bigint", "int"),
        (
            "sqlserver",
            "SELECT app.pick(CAST(1 AS bigint)<cursor>)",
            "int",
            "bigint",
        ),
        (
            "postgresql",
            "SELECT app.pick(CAST(1 AS integer)<cursor>)",
            "text",
            "int4",
        ),
    ] {
        first(dialect, sql, wrong, wanted).await;
    }
}

#[tokio::test]
async fn routine_overloads_rank_before_truncation_and_preserve_arity_priority() {
    let mut functions = (0..60)
        .map(|index| routine(&format!("unknown_{index}"), &["domain_type"]))
        .collect::<Vec<_>>();
    functions.push(routine("wanted", &["integer"]));
    let result = ranked(
        "postgres",
        "/* 😀 */\r\nSELECT app.pick(1<cursor>)",
        functions,
    )
    .await;
    assert_eq!(result.len(), 50);
    assert!(result[0].contains("wanted"), "{result:?}");
    let result = ranked(
        "postgres",
        "SELECT app.pick(1,<cursor>NULL)",
        vec![
            routine("too_short", &["integer"]),
            routine("fits", &["bigint", "integer"]),
        ],
    )
    .await;
    assert!(result[0].contains("fits"), "{result:?}");
    let result = ranked(
        "postgres",
        "SELECT app.pick(1<cursor>, CAST('x' AS text))",
        vec![
            routine("too_short", &["integer"]),
            routine("complete", &["integer", "text"]),
        ],
    )
    .await;
    assert!(result[0].contains("complete"), "{result:?}");
    let mut optional = routine("optional", &["integer"]);
    optional.parameters[0].optional = true;
    let result = ranked(
        "postgres",
        "SELECT app.pick(<cursor>)",
        vec![routine("required", &["integer"]), optional],
    )
    .await;
    assert!(result[0].contains("optional"), "{result:?}");
}

#[test]
fn routine_overload_argument_ranges_keep_raw_trivia_utf16_and_scan_budgets() {
    let sql = "/* 😀 */\r\nSELECT app.pick(/* x,y */ 1, ARRAY[1,2], 'a,b', ";
    let call = routine_call_at_position(sql, lsp_position_at_end(sql), "postgres").unwrap();
    let slices = call
        .arguments
        .iter()
        .map(|range| &sql[range.clone()])
        .collect::<Vec<_>>();
    assert_eq!(slices, vec!["/* x,y */ 1", " ARRAY[1,2]", " 'a,b'", " "]);
    let prefix = "SELECT 1;\r\n".repeat(7000);
    let sql = format!("{prefix}SELECT app.pick(1, ");
    let call = routine_call_at_position(&sql, lsp_position_at_end(&sql), "postgres").unwrap();
    assert_eq!(&sql[call.arguments[0].clone()], "1");
    assert!(call.arguments[0].start > prefix.len());
    let sql = format!("SELECT app.pick({},1", vec!["1"; 256].join(","));
    let call = routine_call_at_position(&sql, lsp_position_at_end(&sql), "postgres").unwrap();
    assert!(call.arguments.is_empty());
    assert_eq!(call.active_parameter, 256);
}

#[tokio::test]
async fn routine_overloads_keep_large_document_analysis_bounded() {
    let sql = format!("{}SELECT app.pick(1<cursor>)", " ".repeat(513 * 1024));
    let started = std::time::Instant::now();
    let result = ranked(
        "postgres",
        &sql,
        vec![
            routine("original", &["text"]),
            routine("other", &["integer"]),
        ],
    )
    .await;
    assert!(result[0].contains("original"));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn routine_overloads_do_not_retain_oversized_metadata_type_names() {
    let mut schema = semantic_test_schema();
    schema.tables[0].columns[0].data_type = "very_long_custom_type".repeat(1000);
    let sql = "SELECT app.pick(u.id) FROM users u";
    let start = sql.find("u.id").unwrap();
    let span = start..start + 4;
    let arguments = super::super::local_relations::infer_routine_argument_types(
        sql,
        std::slice::from_ref(&span),
        &schema,
        "oxide://routine/budget.sql",
        start + 4,
        "postgres",
        None,
    );
    assert_eq!(arguments.types, vec![None]);
    assert_eq!(arguments.complete_arity, Some(1));
}

#[tokio::test]
async fn routine_overloads_do_not_borrow_types_from_invisible_relations() {
    for sql in [
        "WITH unused AS (SELECT id AS secret FROM users) SELECT app.pick(secret<cursor>)",
        "SELECT app.pick(missing.id<cursor>) FROM users u",
        "SELECT 1 FROM users u CROSS JOIN (SELECT app.pick(u.id<cursor>)) isolated",
    ] {
        let result = ranked(
            "postgres",
            sql,
            vec![
                routine("original", &["text"]),
                routine("other", &["bigint"]),
            ],
        )
        .await;
        assert!(result[0].contains("original"), "{sql}: {result:?}");
    }
}
