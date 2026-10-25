use super::*;
use tower_lsp::lsp_types::CompletionResponse;
use tower_lsp::{LanguageServer, LspService};

fn param(name: &str, ty: &str, optional: bool, mode: Option<&str>) -> FunctionParameter {
    FunctionParameter {
        name: name.into(),
        data_type: ty.into(),
        optional,
        mode: mode.map(str::to_owned),
    }
}
fn function(parameters: Vec<FunctionParameter>) -> Function {
    Function {
        name: "calc".into(),
        routine_type: Some("function".into()),
        parameters,
        return_type: "integer".into(),
        description: None,
    }
}
struct Harness {
    service: LspService<crate::server::SqlLspServer>,
    _socket: tower_lsp::ClientSocket,
    uri: Url,
    sql: String,
    position: Position,
}
fn harness(dialect: &str, marked: &str, functions: Vec<Function>) -> Harness {
    let (service, socket) = LspService::new(crate::server::SqlLspServer::new);
    let uri = Url::parse("oxide://named/args.mysql.sql").unwrap();
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
    let cursor = marked.find("<cursor>").unwrap();
    let sql = marked.replace("<cursor>", "");
    service
        .inner()
        .document_manager
        .update(uri.to_string(), sql.clone(), 1);
    Harness {
        service,
        _socket: socket,
        uri,
        sql,
        position: lsp_position_at_end(&marked[..cursor]),
    }
}
async fn help(h: &Harness) -> tower_lsp::lsp_types::SignatureHelp {
    h.service
        .inner()
        .signature_help(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .unwrap()
}
fn two(first: &str, second: &str) -> Function {
    function(vec![
        param("first", first, false, None),
        param("second", second, false, None),
    ])
}
fn hints(h: &Harness) -> Vec<tower_lsp::lsp_types::InlayHint> {
    let schema = h
        .service
        .inner()
        .get_schema_for_file(h.uri.as_str())
        .unwrap();
    super::super::routine_hints::routine_parameter_hints(
        &h.sql,
        range_for_offsets(&h.sql, 0, h.sql.len()),
        &schema,
        &h.service.inner().schema_manager,
        "postgres",
    )
}

#[tokio::test]
async fn named_arguments_map_reordered_active_parameters_in_both_spellings() {
    for op in ["=>", ":="] {
        let h = harness(
            "postgres",
            &format!("SELECT calc(second {op} <cursor>1, first {op} 2)"),
            vec![two("integer", "integer")],
        );
        assert_eq!(help(&h).await.active_parameter, Some(1), "{op}");
    }
}
#[tokio::test]
async fn named_arguments_rank_by_bound_types_including_after_caret_and_query_outputs() {
    for marked in [
        "SELECT calc(second => <cursor>1, first => CAST('x' AS text))",
        "WITH p AS (SELECT id AS value FROM users) SELECT calc(second => <cursor>1, first => p.value) FROM p",
        "SELECT calc(second => <cursor>1, first => (SELECT MAX(id) FROM users))",
    ] {
        let ty=if marked.contains("users") {"bigint"} else {"text"};
        let h=harness("postgres",marked,vec![two("integer",ty),two(ty,"integer")]);
        let result=help(&h).await;
        assert!(result.signatures[0].label.starts_with(&format!("calc(first {ty}, second integer")),"{marked}: {result:?}");
        assert_eq!(result.signatures.len(),2);
        assert_eq!(result.active_parameter,Some(1));
    }
}
#[tokio::test]
async fn named_arguments_omit_any_default_and_keep_required_parameter_checks() {
    let wrong = function(vec![
        param("first", "integer", false, None),
        param("second", "integer", true, None),
    ]);
    let wanted = function(vec![
        param("first", "integer", true, None),
        param("second", "integer", true, None),
    ]);
    let h = harness(
        "postgres",
        "SELECT calc(second => <cursor>1)",
        vec![wrong, wanted],
    );
    assert_eq!(
        help(&h).await.signatures[0].label,
        "calc([first integer], [second integer])"
    );
    let h = harness(
        "postgres",
        "SELECT calc(second => <cursor>1)",
        vec![function(vec![
            param("out", "text", false, Some("OUT")),
            param("first", "integer", true, None),
            param("second", "integer", true, None),
        ])],
    );
    assert_eq!(help(&h).await.active_parameter, Some(1));
}
#[tokio::test]
async fn named_arguments_preserve_quoted_case_escapes_and_unicode() {
    for (name, sql) in [
        ("MiXeD", "SELECT calc(\"MiXeD\" => <cursor>1, first => 2)"),
        (
            "ca\"lc",
            "/* 😀 */\r\nSELECT calc(\"ca\"\"lc\" /* x */ := <cursor>1, first=>2)",
        ),
        ("计算", "SELECT calc(\"计算\" => <cursor>1, first=>2)"),
        ("second", "SELECT calc(SECOND => <cursor>1, first=>2)"),
    ] {
        let h = harness(
            "PGSQL",
            sql,
            vec![function(vec![
                param("first", "integer", false, None),
                param(name, "integer", false, None),
            ])],
        );
        assert_eq!(help(&h).await.active_parameter, Some(1), "{sql}");
    }
}
#[tokio::test]
async fn named_arguments_invalid_bindings_do_not_fabricate_active_slots_or_hints() {
    for sql in [
        "SELECT calc(first=>1, first=><cursor>2)",
        "SELECT calc(first=>1,<cursor>2)",
        "SELECT calc(missing=><cursor>1)",
        "SELECT calc(\"SECOND\"=><cursor>1)",
        "SELECT calc(1,first=><cursor>2)",
    ] {
        let h = harness("postgres", sql, vec![two("integer", "integer")]);
        assert_eq!(help(&h).await.active_parameter, None, "{sql}");
        assert!(hints(&h).is_empty(), "{sql}");
    }
}
#[tokio::test]
async fn named_arguments_inlay_hints_only_label_unnamed_actuals() {
    for sql in [
        "SELECT calc(second=><cursor>2,first=>1)",
        "SELECT calc(\"second\"=><cursor>2,first:=1)",
    ] {
        assert!(
            hints(&harness("postgres", sql, vec![two("integer", "integer")])).is_empty(),
            "{sql}"
        );
    }
    let h = harness(
        "postgres",
        "SELECT calc(1,second=><cursor>2)",
        vec![two("integer", "integer")],
    );
    let result = hints(&h);
    assert_eq!(result.len(), 1, "{result:?}");
    assert!(
        matches!(&result[0].label,tower_lsp::lsp_types::InlayHintLabel::String(label) if label=="first:")
    );
}
#[tokio::test]
async fn named_arguments_completion_excludes_bound_names_and_preserves_existing_values() {
    for (sql, expected) in [
        ("SELECT calc(first=>1,se<cursor>)", "\"second\" => $0"),
        (
            "SELECT calc(first=>1,se<cursor>cond /* keep */ => (2+3))",
            "\"second\"",
        ),
        (
            "SELECT calc(first=>1,\"se<cursor>cond\" := 2)",
            "\"second\"",
        ),
    ] {
        let h = harness("postgres", sql, vec![two("integer", "integer")]);
        let response = h
            .service
            .inner()
            .completion(
                serde_json::from_value(
                    serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
                )
                .unwrap(),
            )
            .await
            .unwrap()
            .unwrap();
        let items = match response {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        };
        let named = items
            .iter()
            .filter(|item| {
                item.kind == Some(tower_lsp::lsp_types::CompletionItemKind::FIELD)
                    && item
                        .detail
                        .as_deref()
                        .is_some_and(|detail| detail.starts_with("Parameter:"))
            })
            .collect::<Vec<_>>();
        assert_eq!(named.len(), 1, "{sql}: {items:?}");
        assert_eq!(named[0].label, "second");
        let Some(tower_lsp::lsp_types::CompletionTextEdit::Edit(edit)) = &named[0].text_edit else {
            panic!("{named:?}")
        };
        assert_eq!(edit.new_text, expected, "{sql}");
        assert!(
            edit.range.start <= h.position && h.position <= edit.range.end,
            "{edit:?}"
        );
    }
}
#[tokio::test]
async fn named_arguments_variadic_name_requires_explicit_array_mode() {
    let f = function(vec![
        param("prefix", "integer", false, None),
        param("items", "integer[]", false, Some("VARIADIC")),
    ]);
    let h = harness(
        "postgres",
        "SELECT calc(prefix=>1,VARIADIC items=><cursor>ARRAY[2,3]::integer[])",
        vec![f.clone()],
    );
    assert_eq!(help(&h).await.active_parameter, Some(1));
    let h = harness(
        "postgres",
        "SELECT calc(prefix=>1,items=><cursor>ARRAY[2,3]::integer[])",
        vec![f],
    );
    assert_eq!(help(&h).await.active_parameter, None);
}
#[test]
fn named_arguments_grammar_retains_real_nodes_and_rejects_foreign_profiles() {
    let mut parser = crate::SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::Postgres,
    );
    for sql in [
        "SELECT calc(second=>1,first:=2);",
        "SELECT calc(\"计算\" /* x */ => (SELECT MAX(id) FROM users));",
        "CALL calc(result=>NULL,value=>1);",
        "SELECT calc(prefix=>1,VARIADIC items=>ARRAY[2,3]::integer[]);",
    ] {
        let parsed = parser.parse(sql);
        assert!(
            parsed.diagnostics.is_empty(),
            "{sql}: {:?}",
            parsed.diagnostics
        );
        let tree = parsed.tree.unwrap();
        assert!(!tree.root_node().has_error(), "{sql}");
        assert!(
            tree.root_node().to_sexp().contains("named_argument"),
            "{}",
            tree.root_node().to_sexp()
        );
    }
    let mut mysql = crate::SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::Mysql,
    );
    assert!(!mysql
        .parse("SELECT calc(second=>1);")
        .diagnostics
        .is_empty());
}

#[test]
fn named_arguments_shared_scanner_keeps_literals_comments_and_budgets() {
    use super::super::routine_arguments::{argument, complete_arguments};
    for sql in [
        "calc('second=>1', /* , ) */ 2)",
        "calc($q$second=>1,)$q$,2)",
        "calc(E'x\\\'second=>1',2)",
    ] {
        let args = complete_arguments(sql, 4, "postgres").unwrap();
        assert_eq!(args.values.len(), 2, "{sql}");
        assert!(!args.has_named(), "{sql}");
    }
    let raw = "first /* nested /* , */ */ => 1";
    assert_eq!(
        argument(raw, 0..raw.len(), "postgres").name.as_deref(),
        Some("first")
    );
    let sql = format!(
        "calc({})",
        (0..257).map(|_| "first=>1").collect::<Vec<_>>().join(",")
    );
    assert!(complete_arguments(&sql, 4, "postgres").is_none());
    let sql = format!("calc(first=>/*{}*/1)", " ".repeat(65536));
    assert!(complete_arguments(&sql, 4, "postgres").is_none());
}

#[tokio::test]
async fn named_arguments_large_parameter_menus_rank_before_the_output_cap() {
    let mut parameters = vec![param("first", "integer", false, None)];
    parameters
        .extend((0..1000).map(|index| param(&format!("se_{index:04}"), "integer", true, None)));
    parameters.push(param("se", "integer", true, None));
    let h = harness(
        "postgres",
        "SELECT calc(first=>1,se<cursor>)",
        vec![function(parameters)],
    );
    let started = std::time::Instant::now();
    let response = h
        .service
        .inner()
        .completion(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    let items = match response {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    };
    let named = items
        .iter()
        .filter(|item| {
            item.detail
                .as_deref()
                .is_some_and(|detail| detail.starts_with("Parameter:"))
        })
        .collect::<Vec<_>>();
    assert_eq!(named.len(), 100);
    assert_eq!(named[0].label, "se");
    let response = h
        .service
        .inner()
        .completion(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(response,CompletionResponse::List(list) if list.is_incomplete));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}
