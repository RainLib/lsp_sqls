use super::*;
use tower_lsp::lsp_types::CompletionResponse;
use tower_lsp::{LanguageServer, LspService};

pub(super) struct Fixture {
    service: LspService<crate::server::SqlLspServer>,
    _socket: tower_lsp::ClientSocket,
    uri: Url,
    pub(super) sql: String,
    position: Position,
}
pub(super) fn fixture(marked: &str, dialect: &str) -> Fixture {
    let (service, socket) = LspService::new(crate::server::SqlLspServer::new);
    let uri = Url::parse("oxide://execute/values.mysql.sql").unwrap();
    let mut schema = semantic_test_schema();
    schema.database = "app".into();
    schema.catalog = Some("db".into());
    schema.functions = vec![Function {
        name: "calculate".into(),
        routine_type: Some("procedure".into()),
        return_type: "void".into(),
        description: None,
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
    }];
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
    Fixture {
        service,
        _socket: socket,
        uri,
        sql,
        position: lsp_position_at_end(&marked[..cursor]),
    }
}
pub(super) async fn response(h: &Fixture) -> CompletionResponse {
    h.service
        .inner()
        .completion(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .unwrap()
}
pub(super) async fn items(h: &Fixture) -> Vec<CompletionItem> {
    match response(h).await {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    }
}
pub(super) fn accepted(h: &Fixture, item: &CompletionItem) -> String {
    let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
        panic!("full edit required: {item:?}")
    };
    let mut sql = h.sql.clone();
    sql.replace_range(
        position_to_byte_offset(&sql, edit.range.start)
            ..position_to_byte_offset(&sql, edit.range.end),
        &edit.new_text,
    );
    sql
}

#[tokio::test]
async fn execute_values_replace_generic_keywords_with_contextual_constants() {
    for dialect in ["sqlserver", "mssql", "tsql"] {
        let h = fixture("EXEC app.calculate @first = <cursor>;", dialect);
        let result = items(&h).await;
        assert_eq!(
            result.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
            vec!["NULL"],
            "{dialect}: {result:?}"
        );
        let h = fixture("EXEC app.calculate 1, @second = DE<cursor>FAULT;", dialect);
        let result = items(&h).await;
        assert_eq!(result.len(), 1, "{result:?}");
        assert_eq!(result[0].label, "DEFAULT");
        assert_eq!(
            accepted(&h, &result[0]),
            "EXEC app.calculate 1, @second = DEFAULT;"
        );
    }
    let h = fixture("EXEC app.calculate @first = CO<cursor>;", "sqlserver");
    assert!(items(&h).await.is_empty());
}

#[tokio::test]
async fn execute_values_offer_declared_variables_with_types_and_complete_edits() {
    let marked="/*😀*/ DECLARE @value DECIMAL(18,2) = COALESCE(1,2), @text AS NVARCHAR(MAX) = CONCAT(N'a,b',N'c');\r\nEXEC app.calculate @first = @va<cursor>lue /*keep*/;";
    let h = fixture(marked, "sqlserver");
    let result = items(&h).await;
    assert_eq!(result.len(), 1, "{result:?}");
    assert_eq!(result[0].label, "@value");
    assert_eq!(result[0].kind, Some(CompletionItemKind::VARIABLE));
    assert!(result[0]
        .detail
        .as_deref()
        .unwrap()
        .contains("DECIMAL(18,2)"));
    assert_eq!(accepted(&h, &result[0]), marked.replace("<cursor>", ""));
    let h = fixture(
        "DECLARE @Mixed int=1; EXEC app.calculate @first = @M<cursor>;",
        "tsql",
    );
    let result = items(&h).await;
    assert_eq!(result[0].label, "@Mixed");
}

#[tokio::test]
async fn execute_values_respect_batch_scope_noise_and_declaration_order() {
    let h=fixture("DECLARE @old int;\r\nGO 2 -- new batch\r\n/*DECLARE @fake int;*/ DECLARE @current int; EXEC app.calculate @first = @<cursor>; DECLARE @future int;","sqlserver");
    assert_eq!(
        items(&h)
            .await
            .iter()
            .map(|i| i.label.as_str())
            .collect::<Vec<_>>(),
        vec!["@current"]
    );
    let h = fixture(
        "DECLARE @old int; SELECT N'GO'; EXEC app.calculate @first = @<cursor>;",
        "sqlserver",
    );
    assert_eq!(
        items(&h)
            .await
            .iter()
            .map(|i| i.label.as_str())
            .collect::<Vec<_>>(),
        vec!["@old"]
    );
    let h = fixture(
        "SELECT N'DECLARE @fake int'; EXEC app.calculate @first = @<cursor>;",
        "sqlserver",
    );
    assert!(items(&h).await.is_empty());
}

#[tokio::test]
async fn execute_values_keep_names_and_values_distinct_after_named_actuals() {
    let h = fixture(
        "DECLARE @value int; EXEC app.calculate @<cursor>;",
        "sqlserver",
    );
    let result = items(&h).await;
    assert!(result.iter().any(|i| i.label == "@first"));
    assert!(result.iter().any(|i| i.label == "@value"));
    let h = fixture(
        "DECLARE @value int; EXEC app.calculate @first=1, @<cursor>;",
        "sqlserver",
    );
    let result = items(&h).await;
    assert_eq!(result.len(), 1, "{result:?}");
    assert_eq!(result[0].label, "@second");
    let h = fixture("EXEC app.calculate @first=1, <cursor>;", "sqlserver");
    let result = items(&h).await;
    assert_eq!(result.len(), 1, "{result:?}");
    assert_eq!(result[0].label, "@second");
}

#[tokio::test]
async fn execute_values_only_offer_output_after_variables_bound_to_output_parameters() {
    let h = fixture(
        "DECLARE @value int; EXEC app.calculate 1, @second = @value O<cursor>UTPUT;",
        "sqlserver",
    );
    let result = items(&h).await;
    assert_eq!(result.len(), 1, "{result:?}");
    assert_eq!(result[0].label, "OUTPUT");
    assert_eq!(accepted(&h, &result[0]), h.sql);
    for sql in [
        "EXEC app.calculate @first = @value O<cursor>;",
        "EXEC app.calculate 1, @second = 1 O<cursor>;",
        "EXEC app.calculate 1, @unknown = @value O<cursor>;",
    ] {
        assert!(items(&fixture(sql, "sqlserver")).await.is_empty(), "{sql}");
    }
}

#[tokio::test]
async fn execute_values_do_not_touch_expression_profiles_and_follow_keyword_preferences() {
    let h = fixture("SELECT CO<cursor>", "sqlserver");
    assert!(items(&h).await.iter().any(|i| i.label == "COALESCE"));
    let h = fixture("SELECT CO<cursor>", "postgres");
    assert!(items(&h).await.iter().any(|i| i.label == "COALESCE"));
    let h = fixture("EXEC app.calculate @first = NU<cursor>LL;", "sqlserver");
    *h.service.inner().completion_preferences.write().unwrap() =
        serde_json::from_value(serde_json::json!({"keywordCase":"lower"})).unwrap();
    let result = items(&h).await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].label, "null");
    assert_eq!(
        accepted(&h, &result[0]),
        "EXEC app.calculate @first = null;"
    );
}

#[test]
fn execute_values_declarations_have_real_nodes_and_keep_syntax_errors() {
    use crate::placeholder::SqlPlaceholderDialect;
    let mut parser =
        crate::SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::SqlServer);
    for sql in ["DECLARE @value int = 1; EXEC app.calculate @first = @value;", "DECLARE @a DECIMAL(18,2)=COALESCE(1,2), @b AS NVARCHAR(MAX)=CONCAT(N'a,b',N'c'); EXEC app.calculate 1, @b OUTPUT;"] {
        let parsed=parser.parse(sql);assert!(parsed.diagnostics.is_empty(),"{sql}: {:?}",parsed.diagnostics);let tree=parsed.tree.unwrap();assert!(!tree.root_node().has_error());assert!(tree.root_node().to_sexp().contains("tsql_declaration"));
    }
    for sql in [
        "DECLARE value int;",
        "DECLARE @value int = ;",
        "DECLARE @value text;",
        "DECLARE @value ntext;",
        "DECLARE @value image;",
        "DECLARE @value int[];",
        "DECLARE @value nchar(MAX);",
    ] {
        assert!(!parser.parse(sql).diagnostics.is_empty(), "{sql}");
    }
    let mut parser =
        crate::SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::Postgres);
    assert!(!parser.parse("DECLARE @value int;").diagnostics.is_empty());
    let parsed=parser.parse("CREATE FUNCTION app.f() RETURNS TRIGGER AS $$ DECLARE n integer; BEGIN RETURN NULL; END $$ LANGUAGE plpgsql;");
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    assert!(!parsed
        .tree
        .unwrap()
        .root_node()
        .to_sexp()
        .contains("tsql_declaration"));
}

#[tokio::test]
async fn execute_values_rank_bound_types_and_scan_all_names_before_candidate_caps() {
    let h = fixture(
        "DECLARE @z_int int, @a_text nvarchar(20); EXEC app.calculate @first = @<cursor>;",
        "sqlserver",
    );
    let result = items(&h).await;
    let variables = result
        .iter()
        .filter(|i| i.kind == Some(CompletionItemKind::VARIABLE))
        .map(|i| i.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(variables, vec!["@z_int", "@a_text"]);
    let declarations = (0..1002)
        .map(|n| format!("DECLARE @pick{n:04} int; "))
        .collect::<String>();
    let h = fixture(
        &format!(
            "{declarations}DECLARE @pick nvarchar(20); EXEC app.calculate @first = @pick<cursor>;"
        ),
        "sqlserver",
    );
    let result = items(&h).await;
    assert_eq!(result.len(), 100);
    assert_eq!(result[0].label, "@pick");
    let raw = h
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
    assert!(
        matches!(raw,CompletionResponse::List(list) if list.is_incomplete && list.items.len()==100)
    );
    let h = fixture(
        "DECLARE @dup int,@dup nvarchar(20); EXEC app.calculate @first = @d<cursor>;",
        "sqlserver",
    );
    assert!(items(&h).await.is_empty());
}

#[tokio::test]
async fn execute_values_metadata_ambiguity_never_guesses_default_or_output_modes() {
    for marked in [
        "EXEC app.calculate 1, @second = DE<cursor>;",
        "EXEC app.calculate 1, @second = @v O<cursor>;",
    ] {
        let h = fixture(marked, "sqlserver");
        let mut schema = h
            .service
            .inner()
            .get_schema_for_file(h.uri.as_str())
            .unwrap();
        let mut duplicate = schema.functions[0].clone();
        duplicate.parameters[1].optional = false;
        duplicate.parameters[1].mode = Some("IN".into());
        schema.functions.push(duplicate);
        h.service.inner().schema_manager.register(schema);
        assert!(items(&h).await.is_empty(), "{marked}");
    }
    for marked in [
        "EXEC app.calculate @first = <cursor> /*keep*/;",
        "EXEC missing @first = <cursor>;",
    ] {
        let h = fixture(marked, "sqlserver");
        let result = items(&h).await;
        assert_eq!(result.len(), 1, "{marked}: {result:?}");
        assert_eq!(result[0].label, "NULL");
        assert!(accepted(&h, &result[0]).contains("= NULL"));
    }
}

#[tokio::test]
async fn execute_values_bound_declaration_work_and_reject_invalid_expression_sites() {
    let h = fixture(
        &format!(
            "/*{}*/ DECLARE @value int; EXEC app.calculate @first = @v<cursor>;",
            "x".repeat(513 * 1024)
        ),
        "sqlserver",
    );
    assert!(items(&h).await.is_empty());
    let h = fixture(
        &format!(
            "/*{}*/\nGO\nDECLARE @value int; EXEC app.calculate @first = @v<cursor>;",
            "x".repeat(513 * 1024)
        ),
        "sqlserver",
    );
    assert_eq!(items(&h).await[0].label, "@value");
    for marked in [
        "EXEC app.calculate @first = COAL<cursor>ESCE(1,2);",
        "EXEC app.calculate @first = N<cursor>'text';",
        "EXEC app.calculate @first = 1+<cursor>2;",
        "EXEC app.calculate @first = N'te<cursor>xt';",
    ] {
        assert!(
            items(&fixture(marked, "sqlserver")).await.is_empty(),
            "{marked}"
        );
    }
}

#[test]
fn tsql_unicode_variables_normalize_whole_names_without_moving_source_bytes() {
    use crate::placeholder::{
        normalize_sql_placeholders_for_dialect, sql_placeholder_spans_for_dialect,
        SqlPlaceholderDialect,
    };
    for name in ["@金额", "@计数$#", "@a@b", "@#临时", "@1", "@Δ٥", "@_"] {
        let sql = format!("/*😀*/ SELECT {name};");
        let spans = sql_placeholder_spans_for_dialect(&sql, SqlPlaceholderDialect::SqlServer);
        assert_eq!(spans.len(), 1, "{name}");
        assert_eq!(&sql[spans[0].start..spans[0].end], name);
        let normalized =
            normalize_sql_placeholders_for_dialect(&sql, SqlPlaceholderDialect::SqlServer);
        assert_eq!(normalized.len(), sql.len());
        assert!(normalized[spans[0].start..spans[0].end].is_ascii());
    }
    let sql = "SELECT N'C:\\', @金额; -- @假\r\n/* @忽略 */ SELECT [列@隐藏], N'@文字', @结果#;";
    let spans = sql_placeholder_spans_for_dialect(sql, SqlPlaceholderDialect::SqlServer);
    assert_eq!(
        spans
            .iter()
            .map(|span| &sql[span.start..span.end])
            .collect::<Vec<_>>(),
        vec!["@金额", "@结果#"]
    );
    assert!(sql_placeholder_spans_for_dialect(
        "SELECT 名@变量, @@VERSION, [@列], '@字';",
        SqlPlaceholderDialect::SqlServer
    )
    .is_empty());
    assert!(
        sql_placeholder_spans_for_dialect("SELECT @金额;", SqlPlaceholderDialect::Postgres)
            .is_empty()
    );
}

#[test]
fn tsql_unicode_variables_real_declarations_and_calls_keep_full_trees() {
    let mut parser = crate::SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::SqlServer,
    );
    for name in ["@金额", "@计数$#", "@a@b", "@#临时", "@1", "@Δ٥"] {
        let sql = format!(
            "/*😀*/ DECLARE {name} int=COALESCE(1,2);\r\nEXEC app.calculate @first={name};"
        );
        let parsed = parser.parse(&sql);
        assert!(
            parsed.diagnostics.is_empty(),
            "{name}: {:?}",
            parsed.diagnostics
        );
        let tree = parsed.tree.unwrap();
        assert!(!tree.root_node().has_error());
        let shape = tree.root_node().to_sexp();
        assert!(
            shape.contains("tsql_declaration") && shape.contains("execute_statement"),
            "{shape}"
        );
    }
    let name = format!("@{}", "值".repeat(127));
    let sql = format!("DECLARE {name} int; EXEC app.calculate @first={name};");
    assert!(parser.parse(&sql).diagnostics.is_empty());
    let overlong = format!("DECLARE @{} int;", "a".repeat(128));
    let diagnostics = parser.parse(&overlong).diagnostics;
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(
        diagnostics[0].code,
        Some(tower_lsp::lsp_types::NumberOrString::String(
            "INVALID_TSQL_VARIABLE".into()
        ))
    );
    for name in [format!("@{}", "a".repeat(128)), "@𐐀".into(), "@½".into()] {
        assert!(
            !parser
                .parse(&format!("DECLARE {name} int;"))
                .diagnostics
                .is_empty(),
            "{name}"
        );
    }
}

#[tokio::test]
async fn tsql_unicode_variables_completion_keeps_sigils_types_and_long_bmp_names() {
    for (name, prefix) in [
        ("@金额", "@金"),
        ("@计数$#", "@计"),
        ("@a@b", "@a@"),
        ("@#临时", "@#"),
        ("@1", "@1"),
        ("@Δ٥", "@Δ"),
    ] {
        let h=fixture(&format!("/*😀*/ DECLARE {name} int=1;\r\n/*😀*/ EXEC app.calculate @first={prefix}<cursor>;"),"sqlserver");
        let result = items(&h).await;
        assert_eq!(result.len(), 1, "{name}: {result:?}");
        assert_eq!(result[0].label, name);
        assert_eq!(result[0].kind, Some(CompletionItemKind::VARIABLE));
        assert!(accepted(&h, &result[0]).ends_with(&format!("@first={name};")));
    }
    let name = format!("@{}", "值".repeat(127));
    let h = fixture(
        &format!("DECLARE {name} int; EXEC app.calculate @first=@值<cursor>;"),
        "mssql",
    );
    let result = items(&h).await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].label, name);
}

#[tokio::test]
async fn tsql_unicode_variables_named_parameters_keep_snippet_escaping_and_signature_slots() {
    let h = fixture(
        "DECLARE @结果# int; EXEC app.calculate @输<cursor>出# = @结果# OUTPUT, @输入$ = 1;",
        "sqlserver",
    );
    let mut schema = h
        .service
        .inner()
        .get_schema_for_file(h.uri.as_str())
        .unwrap();
    schema.functions[0].parameters[0].name = "@输入$".into();
    schema.functions[0].parameters[1].name = "@输出#".into();
    h.service.inner().schema_manager.register(schema);
    let result = items(&h).await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].label, "@输出#");
    assert_eq!(accepted(&h, &result[0]), h.sql);
    let position = lsp_position_at_end(h.sql.split("@结果# OUTPUT").next().unwrap());
    let help = h
        .service
        .inner()
        .signature_help(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":h.uri},"position":position}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(help.active_parameter, Some(1));
    let h = fixture("EXEC app.calculate @入<cursor>", "tsql");
    let mut schema = h
        .service
        .inner()
        .get_schema_for_file(h.uri.as_str())
        .unwrap();
    schema.functions[0].parameters[0].name = "@入参$#".into();
    h.service.inner().schema_manager.register(schema);
    let result = items(&h).await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].insert_text.as_deref(), Some("@入参\\$# = $0"));
}

#[tokio::test]
async fn tsql_unicode_variables_invalid_metadata_names_do_not_become_suggestions() {
    for name in [
        format!("@{}", "a".repeat(128)),
        "@𐐀".into(),
        "@½".into(),
        "@".into(),
        "@@VERSION".into(),
    ] {
        let h = fixture("EXEC app.calculate @<cursor>;", "sqlserver");
        let mut schema = h
            .service
            .inner()
            .get_schema_for_file(h.uri.as_str())
            .unwrap();
        schema.functions[0].parameters[0].name = name.clone();
        h.service.inner().schema_manager.register(schema);
        assert!(
            !items(&h).await.iter().any(|item| item.label == name),
            "{name}"
        );
    }
}
