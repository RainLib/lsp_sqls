use super::*;
use tower_lsp::lsp_types::CompletionResponse;
use tower_lsp::{LanguageServer, LspService};

fn procedure(name: &str, ty: &str) -> Function {
    Function {
        name: name.into(),
        routine_type: Some("procedure".into()),
        parameters: vec![FunctionParameter {
            mode: None,
            name: "value".into(),
            data_type: ty.into(),
            optional: false,
        }],
        return_type: "void".into(),
        description: None,
    }
}
struct Harness {
    service: LspService<crate::server::SqlLspServer>,
    _socket: tower_lsp::ClientSocket,
    uri: Url,
    sql: String,
    position: Position,
    id: SchemaId,
}
fn configured(dialect: &str, marked: &str, functions: Vec<Function>) -> Harness {
    let (service, socket) = LspService::new(crate::server::SqlLspServer::new);
    let uri = Url::parse("oxide://call/target.mysql.sql").unwrap();
    let offset = marked.find("<cursor>").unwrap();
    let sql = marked.replace("<cursor>", "");
    let mut schema = semantic_test_schema();
    schema.database = "app".into();
    schema.catalog = Some("db".into());
    schema.functions = functions;
    let id = service.inner().schema_manager.register(schema);
    service
        .inner()
        .configured_file_schemas
        .insert(uri.to_string(), id);
    service
        .inner()
        .configured_file_dialects
        .insert(uri.to_string(), dialect.into());
    service
        .inner()
        .document_manager
        .update(uri.to_string(), sql.clone(), 1);
    Harness {
        service,
        _socket: socket,
        uri,
        sql,
        position: lsp_position_at_end(&marked[..offset]),
        id,
    }
}
async fn complete(harness: &Harness) -> Vec<CompletionItem> {
    let result = harness
        .service
        .inner()
        .completion(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    match result {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    }
}
fn edit(item: &CompletionItem) -> &TextEdit {
    let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
        panic!("{item:?}")
    };
    edit
}

#[test]
fn call_completion_avoids_loading_metadata_outside_call_targets() {
    for text in ["SELECT calc", "CALL calculate(1, calc"] {
        let result = crate::server::call_completion::completions(
            text,
            lsp_position_at_end(text),
            "postgres",
            || panic!("expression completion must not clone CALL metadata"),
            &SchemaManager::new(),
        );
        assert!(result.is_none(), "{text}");
    }
}

#[tokio::test]
async fn call_completion_offers_only_known_procedures() {
    let mut scalar = procedure("calculate_function", "integer");
    scalar.routine_type = Some("function".into());
    let mut unknown = procedure("calculate_legacy", "integer");
    unknown.routine_type = None;
    let harness = configured(
        "postgres",
        "CALL ca<cursor>",
        vec![procedure("calculate", "integer"), scalar, unknown],
    );
    let items = complete(&harness).await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].label, "calculate");
    assert!(items[0].detail.as_ref().unwrap().starts_with("Procedure:"));
    assert_eq!(edit(&items[0]).new_text, "\"calculate\"($0)");
}

#[tokio::test]
async fn call_completion_ranks_fuzzy_names_and_respects_quoted_case() {
    let harness = configured(
        "postgres",
        "CALL cre<cursor>",
        vec![
            procedure("ClearReports", "integer"),
            procedure("create_report", "integer"),
        ],
    );
    let items = complete(&harness).await;
    assert_eq!(items[0].label, "create_report");
    let harness = configured(
        "postgres",
        "CALL \"Ca<cursor>\"",
        vec![
            procedure("Calculate", "integer"),
            procedure("calculate", "integer"),
        ],
    );
    let items = complete(&harness).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "Calculate");
}

#[tokio::test]
async fn call_completion_uses_unique_qualified_metadata_and_rejects_unknown_scopes() {
    for (marked, duplicate, catalog, expected) in [
        ("CALL analytics.re<cursor>", false, Some("db"), true),
        ("CALL analytics.re<cursor>", true, Some("db"), false),
        ("CALL other.analytics.re<cursor>", false, Some("db"), false),
        ("CALL analytics.re<cursor>", false, None, false),
    ] {
        let harness = configured("postgres", marked, vec![procedure("local", "integer")]);
        let server = harness.service.inner();
        let mut current = server.schema_manager.get(harness.id).unwrap();
        current.catalog = catalog.map(str::to_string);
        server.schema_manager.update(harness.id, current);
        for _ in 0..if duplicate { 2 } else { 1 } {
            let mut remote = semantic_test_schema();
            remote.id = SchemaId::new();
            remote.database = "analytics".into();
            remote.catalog = Some("db".into());
            remote.functions = vec![procedure("remote", "integer")];
            server.schema_manager.register(remote);
        }
        let items = complete(&harness).await;
        assert_eq!(items.len(), usize::from(expected), "{marked}: {items:?}");
        if expected {
            assert_eq!(items[0].label, "remote");
        }
    }
}

#[tokio::test]
async fn call_completion_edits_the_whole_leaf_and_preserves_existing_calls() {
    for (dialect, marked, raw, name) in [
        (
            "postgres",
            "/* 😀 */\r\nCALL app.ca<cursor>lculate /* x */ (1)",
            "calculate",
            "calculate",
        ),
        (
            "postgres",
            "CALL app.\"Ca<cursor>lculate\"(1)",
            "\"Calculate\"",
            "Calculate",
        ),
        (
            "postgres",
            "CALL app.\"ca\"<cursor>\"lc\"(1)",
            "\"ca\"\"lc\"",
            "ca\"lc",
        ),
        (
            "mysql",
            "CALL `app`.`cal<cursor>c``x`(1)",
            "`calc``x`",
            "calc`x",
        ),
        (
            "postgres",
            "CALL app.\"calc<cursor>.with.dot\"(1)",
            "\"calc.with.dot\"",
            "calc.with.dot",
        ),
        (
            "postgres",
            "CALL ca  <cursor> /* keep */(1)",
            "ca  ",
            "calculate",
        ),
    ] {
        let harness = configured(dialect, marked, vec![procedure(name, "integer")]);
        let items = complete(&harness).await;
        let item = items
            .iter()
            .find(|item| item.label == name)
            .unwrap_or_else(|| panic!("{marked}: {items:?}"));
        let start = harness.sql.find(raw).unwrap();
        assert_eq!(
            edit(item).range,
            range_for_offsets(&harness.sql, start, start + raw.len())
        );
        assert!(!edit(item).new_text.contains('('));
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));
    }
}

#[tokio::test]
async fn call_completion_does_not_escape_into_literals_or_scalar_function_lists() {
    for marked in [
        "SELECT 'CALL ca<cursor>'",
        "-- CALL ca<cursor>",
        "CALL calculate('ca<cursor>')",
    ] {
        let harness = configured("postgres", marked, vec![procedure("calculate", "integer")]);
        assert!(
            complete(&harness)
                .await
                .iter()
                .all(|item| item.kind != Some(CompletionItemKind::FUNCTION)),
            "{marked}"
        );
    }
    let mut scalar = procedure("calculate_function", "integer");
    scalar.routine_type = Some("function".into());
    let harness = configured(
        "postgres",
        "SELECT ca<cursor>",
        vec![procedure("calculate", "integer"), scalar],
    );
    let items = complete(&harness).await;
    assert!(
        !items.iter().any(|item| item
            .detail
            .as_deref()
            .is_some_and(|detail| detail.starts_with("Procedure:"))),
        "{items:?}"
    );
    assert!(items.iter().any(|item| item.label == "calculate_function"));
}

#[tokio::test]
async fn call_completion_refreshes_metadata_and_keeps_distinct_overloads() {
    let harness = configured(
        "postgres",
        "CALL cal<cursor>",
        vec![procedure("calc_old", "integer")],
    );
    assert_eq!(complete(&harness).await[0].label, "calc_old");
    let server = harness.service.inner();
    let mut schema = server.schema_manager.get(harness.id).unwrap();
    schema.functions = vec![
        procedure("calc_new", "integer"),
        procedure("calc_new", "text"),
        procedure("calc_new", "text"),
    ];
    server.schema_manager.update(harness.id, schema);
    let items = complete(&harness).await;
    assert_eq!(items.len(), 2, "{items:?}");
    assert!(items.iter().all(|item| item.label == "calc_new"));
    assert_ne!(items[0].detail, items[1].detail);
}

#[tokio::test]
async fn call_completion_closes_incomplete_quotes_without_replacing_later_statements() {
    let harness = configured(
        "postgres",
        "CALL \"ca<cursor>\r\nSELECT 1;",
        vec![procedure("calculate", "integer")],
    );
    let items = complete(&harness).await;
    let item = &items[0];
    assert_eq!(edit(item).range.end, harness.position);
    assert_eq!(edit(item).new_text, "\"calculate\"($0)");
    let harness = configured(
        "postgres",
        "CALL app.<cursor>(1)",
        vec![procedure("calculate", "integer")],
    );
    let items = complete(&harness).await;
    assert_eq!(edit(&items[0]).range.start, harness.position);
    assert_eq!(edit(&items[0]).new_text, "\"calculate\"");
}

#[tokio::test]
async fn call_completion_bounds_the_menu_after_ranking_late_exact_matches() {
    let mut functions = (0..1000)
        .map(|index| procedure(&format!("proc_{index:04}"), "integer"))
        .collect::<Vec<_>>();
    functions.push(procedure("proc_", "integer"));
    let harness = configured("postgres", "/* 😀 */\r\nCALL proc_<cursor>", functions);
    let started = std::time::Instant::now();
    let items = complete(&harness).await;
    assert_eq!(items.len(), 100);
    assert_eq!(items[0].label, "proc_");
    let response = harness
        .service
        .inner()
        .completion(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(response,CompletionResponse::List(list) if list.is_incomplete));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}
