use super::*;
use serde_json::json;
use tower_lsp::LspService;
const URI: &str = "oxide://local/completion.sql";
fn fixture(text: &str) -> LspService<SqlLspServer> {
    let (service, _) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    server.apply_configuration(json!({"oxideAnalysisToken":"warm-private","defaultDialect":"postgres","schemas":[{"id":"00000000-0000-4000-8000-000000000001","database":"private","tables":[{"name":"private_table","columns":[{"name":"secret_value","data_type":"integer","nullable":false}]}],"functions":[{"name":"private_fn","return_type":"integer","parameters":[]}]}]}));
    server.document_manager.update(URI.into(), text.into(), 7);
    service
}
fn params(text: &str, dialect: &str) -> DocumentCompletionParams {
    let at = text.find('|').unwrap();
    let prefix = &text[..at];
    serde_json::from_value(json!({"textDocument":{"uri":URI},"version":7,"dialect":dialect,"position":{"line":prefix.bytes().filter(|b|*b==b'\n').count(),"character":prefix.rsplit('\n').next().unwrap().encode_utf16().count()},"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).unwrap()
}
#[tokio::test]
async fn document_completion_rejects_invalid_utf16_and_outside_document_positions() {
    let text = "/*😀*/ SELECT CAST(1 AS INTEGER) = ";
    let f = fixture(text);
    for position in [
        Position::new(0, 3),
        Position::new(0, text.encode_utf16().count() as u32 + 9),
        Position::new(4, 0),
        Position::new(u32::MAX, u32::MAX),
    ] {
        let mut request = params(&format!("{text}|"), "postgres");
        request.position = position;
        let error = f
            .inner()
            .document_completion_snapshot(request)
            .await
            .expect_err("invalid positions must not acquire type knowledge at a clamped cursor");
        assert_eq!(error.code, ErrorCode::InvalidParams);
    }
}
#[tokio::test]
async fn nested_cte_candidates_keep_types_and_replace_the_entire_suffix() {
    let marked="/*漢😀*/ WITH a AS (SELECT CAST(1 AS INTEGER) AS local_id), q AS (SELECT a.* FROM a) SELECT q.loc|Suffix FROM q;";
    for profile in [
        "postgres",
        "mysql",
        "sqlserver",
        "hive",
        "clickhouse",
        "duckdb",
        "mariadb",
        "mssql",
    ] {
        let text = marked.replace('|', "");
        let f = fixture(&text);
        let result = f
            .inner()
            .document_completion_snapshot(params(marked, profile))
            .await
            .unwrap();
        assert_eq!(result.dialect, profile);
        let item = result
            .items
            .iter()
            .find(|item| item.label == "local_id")
            .unwrap_or_else(|| panic!("{profile}: {:?}", result.items));
        assert!(
            format!("{item:?}").to_uppercase().contains("INTEGER"),
            "{profile}: {item:?}"
        );
        assert!(item.data.is_none() && item.command.is_none());
        let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
            panic!("missing full edit: {item:?}")
        };
        assert!(edit.range.end.character > params(marked, profile).position.character);
        assert!(!edit.new_text.contains("Suffix"));
        assert!(!result
            .items
            .iter()
            .any(|item| item.label.contains("secret_value") || item.label == "private_fn"));
    }
}
#[tokio::test]
async fn document_completion_does_not_wait_for_a_configuration_writer() {
    let marked = "WITH q AS (SELECT 1 AS local_id) SELECT q.lo| FROM q;";
    let f = fixture(&marked.replace('|', ""));
    let _held = f.inner().analysis.configuration.write().await;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        f.inner()
            .document_completion_snapshot(params(marked, "postgres")),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.items.iter().any(|item| item.label == "local_id"));
}
#[tokio::test]
async fn retained_physical_catalog_does_not_fill_an_unknown_star_projection() {
    let marked = "WITH q AS (SELECT * FROM private_table) SELECT q.se| FROM q;";
    let f = fixture(&marked.replace('|', ""));
    assert!(!f
        .inner()
        .document_completion_snapshot(params(marked, "postgres"))
        .await
        .unwrap()
        .items
        .iter()
        .any(|item| item.kind == Some(CompletionItemKind::FIELD)));
}
#[tokio::test]
async fn declared_table_variables_complete_without_catalog_access() {
    let marked = "DECLARE @local TABLE(local_id int); SELECT l.lo| FROM @local l;";
    let f = fixture(&marked.replace('|', ""));
    let text = marked.replace('|', "");
    let shape = local_schema(&text, params(marked, "mssql").position, URI, "sqlserver");
    assert!(!shape.tables.is_empty(), "table shape: {shape:?}");
    assert!(
        f.inner()
            .document_completion_snapshot(params(marked, "mssql"))
            .await
            .unwrap()
            .items
            .iter()
            .any(|item| item.label == "local_id"),
        "shape={shape:?}"
    );
}
#[tokio::test]
async fn caller_preferences_do_not_borrow_the_committed_configuration() {
    let marked = "SEL|";
    let f = fixture(&marked.replace('|', ""));
    let mut p = params(marked, "postgres");
    p.preferences.keyword_case = DocumentCompletionCase::Lower;
    let result = f.inner().document_completion_snapshot(p).await.unwrap();
    assert!(result.items.iter().any(|item| item.label == "select"));
}
#[tokio::test]
async fn invalid_or_stale_requests_do_not_return_empty_success() {
    let marked = "SEL|";
    let f = fixture("SEL");
    for dialect in ["unknown", "POSTGRES", "postgres "] {
        assert!(f
            .inner()
            .document_completion_snapshot(params(marked, dialect))
            .await
            .is_err());
    }
    let mut p = params(marked, "postgres");
    p.version = 6;
    assert_eq!(
        f.inner()
            .document_completion_snapshot(p)
            .await
            .unwrap_err()
            .code,
        ErrorCode::ServerError(-32801)
    );
    f.inner().document_manager.remove(URI);
    assert!(f
        .inner()
        .document_completion_snapshot(params(marked, "postgres"))
        .await
        .is_err());
}
#[test]
fn completion_requests_reject_authority_fields_and_invalid_preferences() {
    let value=serde_json::to_value(json!({"textDocument":{"uri":URI},"version":7,"dialect":"postgres","position":{"line":0,"character":0},"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).unwrap();
    for key in ["schemas", "oxideAnalysisToken", "project", "fileDialects"] {
        let mut p = value.clone();
        p[key] = json!("forbidden");
        assert!(serde_json::from_value::<DocumentCompletionParams>(p).is_err());
    }
    let mut p = value;
    p["preferences"]["keywordCase"] = json!("auto");
    assert!(serde_json::from_value::<DocumentCompletionParams>(p).is_err());
}

#[tokio::test]
async fn table_variable_shapes_preserve_types_quotes_and_real_constraints() {
    let marked = "DECLARE @local AS TABLE([列 名] nvarchar(42) DEFAULT 'NOT NULL UNIQUE', amount decimal(19,4) NOT NULL, id int PRIMARY KEY, CONSTRAINT pk UNIQUE(amount)); SELECT l.| FROM @local l;";
    let text = marked.replace('|', "");
    let schema = local_schema(&text, params(marked, "mssql").position, URI, "sqlserver");
    let table = schema
        .tables
        .iter()
        .find(|value| value.name == "@local")
        .unwrap();
    assert_eq!(table.columns.len(), 3, "{table:?}");
    assert_eq!(table.columns[0].name, "列 名");
    assert_eq!(table.columns[0].data_type.to_uppercase(), "NVARCHAR(42)");
    assert!(table.columns[0].nullable && !table.columns[0].unique);
    assert_eq!(table.columns[1].data_type.to_uppercase(), "DECIMAL(19,4)");
    assert!(!table.columns[1].nullable);
    assert!(table.columns[2].primary_key);
    assert!(!table.columns[2].nullable);
    let result = fixture(&text)
        .inner()
        .document_completion_snapshot(params(marked, "mssql"))
        .await
        .unwrap();
    assert!(result.items.iter().any(|item| item.label == "列 名"));
}

#[tokio::test]
async fn table_variables_obey_batch_module_duplicate_and_forward_boundaries() {
    for marked in [
        "DECLARE @local TABLE(local_id int);\nGO\nSELECT l.lo| FROM @local l;",
        "DECLARE @local TABLE(local_id int); DECLARE @local TABLE(other int); SELECT l.lo| FROM @local l;",
        "SELECT l.lo| FROM @local l; DECLARE @local TABLE(local_id int);",
        "DECLARE @local TABLE(local_id int); CREATE PROCEDURE p AS SELECT l.lo| FROM @local l;",
        "-- DECLARE @local TABLE(local_id int);\nSELECT l.lo| FROM @local l;",
    ] {
        let f = fixture(&marked.replace('|', ""));
        let result = f.inner().document_completion_snapshot(params(marked, "sqlserver")).await.unwrap();
        assert!(!result.items.iter().any(|item| item.label == "local_id"), "{marked}: {:?}", result.items);
    }
}

#[tokio::test]
async fn table_variable_join_aliases_do_not_mix_fields_and_non_tsql_does_not_infer_them() {
    let marked = "DECLARE @left TABLE(left_id int); DECLARE @right TABLE(right_id int); SELECT r.| FROM @left l JOIN @right r ON 1=1;";
    for dialect in ["sqlserver", "postgres", "mysql", "duckdb"] {
        let f = fixture(&marked.replace('|', ""));
        let result = f
            .inner()
            .document_completion_snapshot(params(marked, dialect))
            .await
            .unwrap();
        assert_eq!(
            result.items.iter().any(|item| item.label == "right_id"),
            dialect == "sqlserver",
            "{dialect}: {:?}",
            result.items
        );
        assert!(!result.items.iter().any(|item| item.label == "left_id"));
    }
}

#[tokio::test]
async fn large_declarations_rank_late_exact_fields_before_bounded_wire_truncation() {
    let columns = (0..1024)
        .map(|n| format!("field_{n:04} int"))
        .collect::<Vec<_>>()
        .join(",");
    let marked =
        format!("DECLARE @local TABLE({columns}); SELECT l.field_102|Suffix FROM @local l;");
    let f = fixture(&marked.replace('|', ""));
    let result = f
        .inner()
        .document_completion_snapshot(params(&marked, "sqlserver"))
        .await
        .unwrap();
    assert!(result.items.len() <= 512);
    assert!(
        result.items.iter().any(|item| item.label == "field_1023"),
        "late declared field must survive prefix ranking"
    );
}

#[tokio::test]
async fn oversized_documents_fail_explicitly_instead_of_claiming_empty_completion() {
    let marked = format!("/*{}*/ SEL|", "x".repeat(512 * 1024));
    let f = fixture(&marked.replace('|', ""));
    assert_eq!(
        f.inner()
            .document_completion_snapshot(params(&marked, "postgres"))
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidParams
    );
}

#[test]
fn quoted_constraint_words_are_columns_in_temporary_tables_and_table_variables() {
    for declaration in [
        "DECLARE @local TABLE([UNIQUE] int, [PRIMARY] int, [CHECK] int, [CONSTRAINT] int, [FOREIGN] int); SELECT l.* FROM @local l;",
        "CREATE TEMP TABLE local(\"UNIQUE\" int, \"PRIMARY\" int, \"CHECK\" int, \"CONSTRAINT\" int, \"FOREIGN\" int); SELECT l.* FROM local l;",
    ] {
        let dialect = if declaration.starts_with("DECLARE") { "sqlserver" } else { "postgres" };
        let position = Position::new(0, declaration.len() as u32);
        let schema = local_schema(declaration, position, URI, dialect);
        let table = schema.tables.iter().find(|table| table.name == "@local" || table.name == "local").unwrap();
        assert_eq!(table.columns.iter().map(|column| column.name.as_str()).collect::<Vec<_>>(), vec!["UNIQUE", "PRIMARY", "CHECK", "CONSTRAINT", "FOREIGN"], "{dialect}: {table:?}");
    }
}

#[test]
fn local_column_constraint_flags_ignore_names_expressions_and_opaque_text() {
    let definitions = [
        ("unique_count int", true, false, false),
        ("[NOT NULL] int", true, false, false),
        (
            "value int DEFAULT 'NOT NULL PRIMARY KEY UNIQUE'",
            true,
            false,
            false,
        ),
        ("value int CHECK (value IS NOT NULL)", true, false, false),
        (
            "value int DEFAULT CASE WHEN 1 IS NOT NULL THEN 1 ELSE NULL END",
            true,
            false,
            false,
        ),
        (
            "value int CONSTRAINT [UNIQUE] CHECK (value IS NOT NULL)",
            true,
            false,
            false,
        ),
        ("value int NOT /* 漢😀 */ NULL", false, false, false),
        ("value int PRIMARY /* ignored */ KEY", true, true, false),
        ("value int UNIQUE", true, false, true),
    ];
    for (definition, nullable, primary_key, unique) in definitions {
        let column =
            super::super::local_relations::parse_temporary_column(definition, URI, 1).unwrap();
        assert_eq!(
            (column.nullable, column.primary_key, column.unique),
            (nullable, primary_key, unique),
            "{definition}"
        );
    }
}
