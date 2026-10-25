use super::*;
use tower_lsp::{LanguageServer, LspService};

fn parameter(name: &str, ty: &str, mode: Option<&str>) -> serde_json::Value {
    serde_json::json!({"name":name,"data_type":ty,"optional":false,"mode":mode})
}
fn routine(kind: &str, params: Vec<serde_json::Value>) -> Function {
    serde_json::from_value(serde_json::json!({"name":"calc","routine_type":kind,"parameters":params,"return_type":"integer","description":null})).unwrap()
}
async fn signature(
    dialect: &str,
    marked: &str,
    functions: Vec<Function>,
) -> tower_lsp::lsp_types::SignatureHelp {
    let (service, _socket) = LspService::new(crate::server::SqlLspServer::new);
    let server = service.inner();
    let uri = Url::parse("oxide://routine/modes.mysql.sql").unwrap();
    let mut metadata = semantic_test_schema();
    metadata.functions = functions;
    let id = server.schema_manager.register(metadata);
    server.configured_file_schemas.insert(uri.to_string(), id);
    server
        .configured_file_dialects
        .insert(uri.to_string(), dialect.into());
    let offset = marked.find("<cursor>").unwrap();
    server
        .document_manager
        .update(uri.to_string(), marked.replace("<cursor>", ""), 1);
    server.signature_help(serde_json::from_value(serde_json::json!({"textDocument":{"uri":uri},"position":lsp_position_at_end(&marked[..offset])})).unwrap()).await.unwrap().unwrap()
}

#[test]
fn routine_modes_survive_json_and_legacy_parameters_remain_compatible() {
    for mode in ["IN", "OUT", "INOUT", "VARIADIC", "TABLE", "vendor_mode"] {
        let function = routine("function", vec![parameter("x", "integer", Some(mode))]);
        assert_eq!(
            serde_json::to_value(function).unwrap()["parameters"][0]["mode"],
            mode
        );
    }
    let parameter: FunctionParameter =
        serde_json::from_value(serde_json::json!({"name":"x","data_type":"int","optional":false}))
            .unwrap();
    assert_eq!(parameter.signature_label(), "x int");
}

#[tokio::test]
async fn routine_modes_postgres_functions_skip_output_only_slots() {
    let help = signature(
        "postgres",
        "SELECT calc(1,<cursor>2)",
        vec![routine(
            "function",
            vec![
                parameter("x", "integer", Some("IN")),
                parameter("total", "bigint", Some("OUT")),
                parameter("acc", "integer", Some("INOUT")),
                parameter("row_name", "text", Some("TABLE")),
            ],
        )],
    )
    .await;
    assert_eq!(
        help.signatures[0].label,
        "calc(x integer, INOUT acc integer)"
    );
    assert_eq!(help.signatures[0].parameters.as_ref().unwrap().len(), 2);
    assert_eq!(help.active_parameter, Some(1));
    assert!(format!("{:?}", help.signatures[0].documentation).contains("OUT total"));
    let help = signature(
        "postgres",
        "SELECT calc(<cursor>)",
        vec![routine(
            "function",
            vec![parameter("out", "integer", Some("OUT"))],
        )],
    )
    .await;
    assert_eq!(help.signatures[0].label, "calc()");
    assert_eq!(help.active_parameter, None);
    for dialect in ["postgresql", "PGSQL", "POSTGRES"] {
        let help = signature(
            dialect,
            "SELECT calc(<cursor>1)",
            vec![routine(
                "function",
                vec![
                    parameter("out", "integer", Some("OUT")),
                    parameter("value", "integer", None),
                ],
            )],
        )
        .await;
        assert_eq!(help.signatures[0].label, "calc(value integer)", "{dialect}");
    }
}

#[tokio::test]
async fn routine_modes_procedure_outputs_still_take_actual_argument_slots() {
    for dialect in ["postgres", "mysql", "sqlserver"] {
        let help = signature(
            dialect,
            "CALL calc(1,<cursor>NULL)",
            vec![routine(
                "procedure",
                vec![
                    parameter("x", "integer", Some("IN")),
                    parameter("total", "integer", Some("OUT")),
                ],
            )],
        )
        .await;
        assert_eq!(
            help.signatures[0].label, "calc(x integer, OUT total integer)",
            "{dialect}"
        );
        assert_eq!(help.signatures[0].parameters.as_ref().unwrap().len(), 2);
        assert_eq!(help.active_parameter, Some(1));
    }
}

#[tokio::test]
async fn routine_modes_expanded_variadics_rank_by_element_type_and_clamp_active_slot() {
    let fixed = routine("function", vec![parameter("fixed", "integer", None)]);
    let variadic = routine(
        "function",
        vec![parameter("items", "integer[]", Some("VARIADIC"))],
    );
    let help = signature(
        "postgres",
        "SELECT calc(1,2,<cursor>3)",
        vec![fixed, variadic],
    )
    .await;
    assert_eq!(help.signatures[0].label, "calc(VARIADIC items integer[])");
    assert_eq!(help.active_parameter, Some(0));
    assert_eq!(help.signatures.len(), 2);
}

#[test]
fn routine_modes_inlay_hints_skip_outputs_and_repeat_variadic_element_labels() {
    let mut metadata = semantic_test_schema();
    metadata.functions = vec![routine(
        "function",
        vec![
            parameter("out", "integer", Some("OUT")),
            parameter("id", "integer", Some("INOUT")),
            parameter("items", "integer[]", Some("VARIADIC")),
        ],
    )];
    let sql = "/* 😀 */\r\nSELECT calc(1, 2, COALESCE(NULL,3));";
    let hints = super::super::routine_hints::routine_parameter_hints(
        sql,
        range_for_offsets(sql, 0, sql.len()),
        &metadata,
        &SchemaManager::new(),
        "postgres",
    );
    let labels = hints
        .iter()
        .map(|hint| match &hint.label {
            InlayHintLabel::String(label) => label.as_str(),
            _ => "parts",
        })
        .collect::<Vec<_>>();
    assert_eq!(labels, vec!["id:", "items:", "items:"]);
    assert!(format!("{:?}", hints[2].tooltip).contains("Parameter type: integer"));
}

#[tokio::test]
async fn routine_modes_unknown_modes_and_nonarray_variadics_stay_fixed() {
    let help = signature(
        "postgres",
        "SELECT calc(1,<cursor>2)",
        vec![routine(
            "function",
            vec![
                parameter("unknown", "integer", Some("vendor_mode")),
                parameter("next", "integer", None),
            ],
        )],
    )
    .await;
    assert_eq!(help.signatures[0].parameters.as_ref().unwrap().len(), 2);
    let help = signature(
        "mysql",
        "SELECT calc(1,<cursor>2)",
        vec![routine(
            "function",
            vec![parameter("single", "integer[]", Some("VARIADIC"))],
        )],
    )
    .await;
    assert_eq!(help.signatures[0].parameters.as_ref().unwrap().len(), 1);
    let help = signature(
        "postgres",
        "SELECT calc(1,<cursor>2)",
        vec![
            routine(
                "function",
                vec![parameter("invalid", "integer", Some("VARIADIC"))],
            ),
            routine(
                "function",
                vec![
                    parameter("a", "integer", None),
                    parameter("b", "integer", None),
                ],
            ),
        ],
    )
    .await;
    assert_eq!(help.signatures[0].label, "calc(a integer, b integer)");
}

#[tokio::test]
async fn routine_modes_explicit_variadic_prefers_array_mode_even_before_keyword() {
    for marked in [
        "SELECT calc(VARIADIC ARRAY[1,2]::integer[]<cursor>)",
        "SELECT calc(<cursor>VARIADIC ARRAY[1,2]::integer[])",
    ] {
        let help = signature(
            "postgres",
            marked,
            vec![
                routine(
                    "function",
                    vec![parameter("ordinary_array", "integer[]", None)],
                ),
                routine(
                    "function",
                    vec![parameter("items", "integer[]", Some("VARIADIC"))],
                ),
            ],
        )
        .await;
        assert_eq!(
            help.signatures[0].label, "calc(VARIADIC items integer[])",
            "{marked}"
        );
    }
}

#[tokio::test]
async fn routine_modes_call_and_nested_function_choose_their_own_kind() {
    for (sql, kind) in [
        ("CALL /* x */ calc(<cursor>1)", "OUT"),
        ("SELECT calc(<cursor>1)", "INOUT"),
        ("CALL calc(calc(<cursor>1))", "INOUT"),
    ] {
        let help = signature(
            "postgres",
            sql,
            vec![
                routine(
                    "procedure",
                    vec![parameter("result", "integer", Some("OUT"))],
                ),
                routine(
                    "function",
                    vec![parameter("value", "integer", Some("INOUT"))],
                ),
            ],
        )
        .await;
        assert_eq!(help.signatures.len(), 1, "{sql}: {help:?}");
        assert!(help.signatures[0].label.contains(kind), "{sql}: {help:?}");
    }
}

#[test]
fn routine_modes_variadic_invocations_have_real_trees_and_dialect_restrictions() {
    let mut parser = crate::SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::Postgres,
    );
    for sql in [
        "SELECT calc(VARIADIC ARRAY[1,2]::integer[]);",
        "SELECT calc(1, VARIADIC ARRAY[]::integer[]);",
        "SELECT calc(VARIADIC /* x */ ARRAY[1]);",
    ] {
        let parsed = parser.parse(sql);
        assert!(
            parsed.diagnostics.is_empty(),
            "{sql}: {:?}",
            parsed.diagnostics
        );
        assert!(!parsed.tree.unwrap().root_node().has_error(), "{sql}");
    }
    for sql in [
        "SELECT calc(VARIADIC ARRAY[1],2);",
        "SELECT calc(VARIADIC ARRAY[1],VARIADIC ARRAY[2]);",
    ] {
        assert!(!parser.parse(sql).diagnostics.is_empty(), "{sql}");
    }
    let mut mysql = crate::SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::Mysql,
    );
    assert!(!mysql
        .parse("SELECT calc(VARIADIC ARRAY[1]);")
        .diagnostics
        .is_empty());
}
