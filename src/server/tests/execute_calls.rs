use super::*;
use tower_lsp::lsp_types::{CompletionResponse, InlayHint, NumberOrString, SignatureHelp};
use tower_lsp::{LanguageServer, LspService};

fn procedure() -> Function {
    Function {
        name: "calculate".into(),
        routine_type: Some("procedure".into()),
        parameters: vec![
            FunctionParameter {
                name: "@first".into(),
                data_type: "int".into(),
                optional: false,
                mode: Some("IN".into()),
            },
            FunctionParameter {
                name: "@second".into(),
                data_type: "nvarchar".into(),
                optional: true,
                mode: Some("OUT".into()),
            },
        ],
        return_type: "void".into(),
        description: None,
    }
}
async fn inspect(
    marked: &str,
    dialect: &str,
) -> (Option<SignatureHelp>, Vec<CompletionItem>, Vec<InlayHint>) {
    let (service, _socket) = LspService::new(crate::server::SqlLspServer::new);
    let uri = Url::parse("oxide://execute/args.mysql.sql").unwrap();
    let mut schema = semantic_test_schema();
    schema.database = "app".into();
    schema.catalog = Some("db".into());
    let mut function = procedure();
    function.routine_type = Some("function".into());
    function.name = "callable_function".into();
    schema.functions = vec![procedure(), function];
    let id = service.inner().schema_manager.register(schema.clone());
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
    let position = lsp_position_at_end(&marked[..cursor]);
    service
        .inner()
        .document_manager
        .update(uri.to_string(), sql.clone(), 1);
    let params = serde_json::json!({"textDocument":{"uri":uri},"position":position});
    let help = service
        .inner()
        .signature_help(serde_json::from_value(params.clone()).unwrap())
        .await
        .unwrap();
    let completion = service
        .inner()
        .completion(serde_json::from_value(params).unwrap())
        .await
        .unwrap();
    let items = match completion {
        Some(CompletionResponse::Array(items)) => items,
        Some(CompletionResponse::List(list)) => list.items,
        None => vec![],
    };
    let hints = super::super::routine_hints::routine_parameter_hints(
        &sql,
        range_for_offsets(&sql, 0, sql.len()),
        &schema,
        &service.inner().schema_manager,
        dialect,
    );
    (help, items, hints)
}

#[tokio::test]
async fn execute_calls_bind_named_reordered_and_mixed_arguments() {
    for dialect in ["sqlserver", "mssql", "tsql"] {
        for marked in [
            "EXEC app.calculate @second = <cursor>N'a,b', @first = 1;",
            "EXECUTE @status = [db].[app].[calculate] 1, @second /*x*/ = <cursor>@value OUTPUT;",
            "/*😀*/ EXEC app.calculate @second = <cursor>DEFAULT, @first = -1;",
        ] {
            let help = inspect(marked, dialect).await.0.expect(marked);
            assert_eq!(help.active_parameter, Some(1), "{dialect}: {marked}");
            assert_eq!(help.signatures[0].parameters.as_ref().unwrap().len(), 2);
        }
    }
}

#[tokio::test]
async fn execute_calls_target_edit_never_inserts_parentheses_or_consumes_arguments() {
    for (marked, expected) in [
        (
            "EXEC app.ca<cursor>lculate 1, @second = @v OUTPUT;",
            "[calculate]",
        ),
        (
            "EXECUTE @status = [db].[app].[ca<cursor>lculate] /* x */ 1;",
            "[calculate]",
        ),
        ("EXEC ca<cursor>", "[calculate]"),
        ("EXEC <cursor>", "[calculate]"),
        (
            "DECLARE @s int\nEXECUTE @s = app.ca<cursor>lculate 1;",
            "[calculate]",
        ),
    ] {
        let items = inspect(marked, "sqlserver").await.1;
        assert_eq!(items.len(), 1, "{marked}: {items:?}");
        let Some(CompletionTextEdit::Edit(edit)) = &items[0].text_edit else {
            panic!("missing edit")
        };
        assert_eq!(edit.new_text, expected);
        assert_eq!(
            items[0].insert_text_format,
            Some(InsertTextFormat::PLAIN_TEXT)
        );
    }
}

#[tokio::test]
async fn execute_calls_name_menu_preserves_equals_comments_and_output() {
    for (marked, expected, format) in [
        (
            "EXEC app.calculate 1, @se<cursor>cond /*x*/ = @value OUTPUT;",
            "@second",
            InsertTextFormat::PLAIN_TEXT,
        ),
        (
            "EXEC app.calculate @se<cursor>",
            "@second = $0",
            InsertTextFormat::SNIPPET,
        ),
    ] {
        let items = inspect(marked, "sqlserver").await.1;
        let named = items
            .iter()
            .filter(|item| {
                item.sort_text
                    .as_deref()
                    .is_some_and(|key| key.starts_with("0:parameter:"))
            })
            .collect::<Vec<_>>();
        assert_eq!(named.len(), 1, "{items:?}");
        assert_eq!(named[0].label, "@second");
        let Some(CompletionTextEdit::Edit(edit)) = &named[0].text_edit else {
            panic!("missing edit")
        };
        assert_eq!(edit.new_text, expected);
        assert_eq!(named[0].insert_text_format, Some(format));
    }
}

#[tokio::test]
async fn execute_calls_hints_label_only_positional_arguments() {
    let (_, _, hints) = inspect("EXEC app.calculate <cursor>1, @value OUTPUT;", "sqlserver").await;
    assert_eq!(hints.len(), 2, "{hints:?}");
    assert!(matches!(&hints[0].label, InlayHintLabel::String(label) if label == "@first:"));
    assert!(matches!(&hints[1].label, InlayHintLabel::String(label) if label == "@second:"));
    assert!(inspect(
        "EXEC app.calculate @first = <cursor>1, @second = @v OUT;",
        "tsql"
    )
    .await
    .2
    .is_empty());
    let (_, _, hints) = inspect(
        "/*😀*/ EXEC app.calculate <cursor>1, @value OUTPUT;\r\nEXEC app.calculate 2, @next OUT;",
        "sqlserver",
    )
    .await;
    assert_eq!(
        hints.len(),
        4,
        "statement boundaries must not duplicate hints: {hints:?}"
    );
}

#[tokio::test]
async fn execute_calls_invalid_bindings_do_not_guess_active_slots() {
    for sql in [
        "EXEC app.calculate @first = 1, @first = <cursor>2;",
        "EXEC app.calculate @second = @v, <cursor>1;",
        "EXEC app.calculate @unknown = <cursor>1;",
    ] {
        let (help, _, hints) = inspect(sql, "sqlserver").await;
        assert_eq!(help.expect(sql).active_parameter, None, "{sql}");
        assert!(hints.is_empty());
    }
}

#[tokio::test]
async fn execute_calls_ignore_dynamic_sql_foreign_dialects_and_other_statements() {
    for sql in [
        "SELECT 'EXEC app.calculate 1, <cursor>2';",
        "-- EXEC app.calculate 1, <cursor>2",
        "/* EXEC app.calculate 1, <cursor>2 */",
        "EXEC ('EXEC app.calculate 1, <cursor>2');",
        "EXEC @module 1, <cursor>2;",
        "EXEC app.calculate 1; SELECT <cursor>2;",
        "EXEC app.calculate 1\nSELECT <cursor>2;",
    ] {
        assert!(inspect(sql, "sqlserver").await.0.is_none(), "{sql}");
    }
    for dialect in ["postgres", "mysql", "sqlite", "clickhouse"] {
        assert!(
            inspect("EXEC app.calculate 1, <cursor>2;", dialect)
                .await
                .0
                .is_none(),
            "{dialect}"
        );
    }
}

#[test]
fn execute_calls_grammar_retains_nodes_and_foreign_profile_errors() {
    let mut parser = crate::SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::SqlServer,
    );
    for sql in [
        "EXEC app.calculate 1, @value OUTPUT;",
        "EXECUTE @status = [db].[app].[calculate] @second = N'a,b', @first = -1;",
        "EXEC app.calculate @first = DEFAULT WITH RECOMPILE;",
        "EXEC app.calculate; SELECT 1;",
    ] {
        let result = parser.parse(sql);
        assert!(
            result.diagnostics.is_empty(),
            "{sql}: {:?}",
            result.diagnostics
        );
        let tree = result.tree.unwrap();
        assert!(tree.root_node().to_sexp().contains("execute_statement"));
        assert!(!tree.root_node().has_error());
    }
    for dialect in [
        crate::placeholder::SqlPlaceholderDialect::Postgres,
        crate::placeholder::SqlPlaceholderDialect::Mysql,
    ] {
        let mut parser = crate::SqlParser::new_with_placeholder_dialect(dialect);
        assert!(parser
            .parse("EXEC app.calculate 1;")
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code
                == Some(NumberOrString::String("UNSUPPORTED_DIALECT_SYNTAX".into()))));
    }
    assert!(!parser
        .parse("EXEC app.calculate @first = ;")
        .diagnostics
        .is_empty());
    assert!(!parser
        .parse("EXEC app.calculate (1+2);")
        .diagnostics
        .is_empty());
    for sql in [
        "EXEC status = app.calculate 1;",
        "EXEC app.calculate first = 1;",
        "EXEC app.calculate 1 OUTPUT;",
    ] {
        assert!(
            parser
                .parse(sql)
                .diagnostics
                .iter()
                .any(|d| d.code == Some(NumberOrString::String("INVALID_EXECUTE_ARGUMENT".into()))),
            "{sql}"
        );
    }
}

#[tokio::test]
async fn execute_calls_procedures_never_fall_back_to_builtin_function_signatures() {
    for (dialect, sql) in [
        ("sqlserver", "EXEC SUM <cursor>1;"),
        ("tsql", "CALL SUM(<cursor>1);"),
        ("postgres", "CALL sum(<cursor>1);"),
    ] {
        assert!(inspect(sql, dialect).await.0.is_none(), "{dialect}: {sql}");
    }
    for dialect in ["postgres", "sqlserver"] {
        assert!(
            inspect("SELECT SUM(<cursor>1)", dialect).await.0.is_some(),
            "{dialect}"
        );
    }
}

#[test]
fn execute_calls_shared_scanner_bounds_and_statement_noise() {
    let scan = |sql: &str| {
        super::super::routine_context::routine_call_at_position(
            sql,
            lsp_position_at_end(sql),
            "sqlserver",
        )
    };
    assert!(scan(&format!("/*{} EXEC app.calculate 1, ", "x".repeat(70_000))).is_none());
    assert!(scan(&format!("EXEC app.calculate {}", "1,".repeat(257))).is_none());
    assert!(scan(&format!(
        "EXEC app.calculate /*{}*/ 1, ",
        "x".repeat(70_000)
    ))
    .is_none());
    let call = scan(
        "SELECT ';';\r\n/*😀*/ EXECUTE @s /* = */ = app.calculate @first = N'a,;WITH', @second = ",
    )
    .unwrap();
    assert_eq!(call.name, "app.calculate");
    assert_eq!(call.active_parameter, 1);
    assert_eq!(call.supplied.values[0].name.as_deref(), Some("@first"));
    assert_eq!(call.supplied.values[1].name.as_deref(), Some("@second"));
    assert_eq!(
        scan("EXEC app.calculate @WITHIN = ")
            .unwrap()
            .active_parameter,
        0
    );
    assert!(scan("EXEC app.calculate 1 WITH RECOMPILE").is_none());
    let batch =
        scan("DECLARE @s int\nEXEC app.calculate 1\nEXECUTE @s = app.calculate 2, ").unwrap();
    assert_eq!(batch.name, "app.calculate");
    assert_eq!(batch.active_parameter, 1);
}
