use super::*;
#[tokio::test]
async fn duckdb_local_cast_hover_uses_its_registered_ansi_adapter() {
    for (expression, expected) in [
        ("CAST(1 AS INTEGER)", "INTEGER"),
        ("TRY_CAST('bad' AS INTEGER)", "INTEGER"),
        ("CAST('漢😀' AS VARCHAR)", "VARCHAR"),
        ("CAST(9007199254740993 AS BIGINT)", "BIGINT"),
    ] {
        let text = format!("WITH q AS (SELECT {expression} AS local_count) SELECT q.local_count FROM q; SELECT d.label FROM data d;");
        let f = fixture(&text);
        let mut p = request(json!({"kind":"hover","position":at(&text,"q.local_count")}));
        p.dialect = "duckdb".into();
        let result = f.inner().document_semantic_snapshot(p).await.unwrap();
        assert!(
            result.value.to_string().to_uppercase().contains(expected),
            "{expression}: {}",
            result.value
        );
        let mut p =
            request(json!({"kind":"rename","position":at(&text,"FROM q"),"newName":"new_q"}));
        p.dialect = "duckdb".into();
        assert_eq!(
            f.inner().document_semantic_snapshot(p).await.unwrap().value["changes"][URI]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }
}
#[tokio::test]
async fn reviewed_alias_profiles_keep_local_hover_and_cte_binding() {
    let text = "WITH q AS (SELECT 1 AS local_id) SELECT q.local_id FROM q;";
    let f = fixture(text);
    for dialect in [
        "duckdb",
        "mariadb",
        "postgresql",
        "mssql",
        "argo",
        "turso",
        "tidb",
        "cockroachdb",
    ] {
        let mut p = request(json!({"kind":"hover","position":at(text,"q.local_id")}));
        p.dialect = dialect.into();
        let result = f.inner().document_semantic_snapshot(p).await.unwrap();
        assert_eq!(result.dialect, dialect);
        assert!(!result.value.is_null(), "{dialect}");
        let mut p = request(json!({"kind":"definitions","position":at(text,"FROM q")}));
        p.dialect = dialect.into();
        assert!(
            !f.inner()
                .document_semantic_snapshot(p)
                .await
                .unwrap()
                .value
                .as_array()
                .unwrap()
                .is_empty(),
            "{dialect}"
        );
    }
}
#[tokio::test]
async fn a_column_with_a_cte_name_is_not_a_cte_rename_target() {
    let text = "WITH q AS (SELECT 1 AS q) SELECT q.q FROM q;";
    let f = fixture(text);
    let position = position_for_offset(text, text.find("q.q").unwrap() + 2);
    let uri: Url = URI.parse().unwrap();
    let edit = f
        .inner()
        .rename(RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            new_name: "fresh_q".into(),
            work_done_progress_params: Default::default(),
        })
        .await
        .unwrap();
    assert!(edit.is_none());
    let prepared = f
        .inner()
        .prepare_rename(TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        })
        .await
        .unwrap();
    assert!(prepared.is_none());
}
use serde_json::{json, Value};
use tower_lsp::{LanguageServer, LspService};
const URI: &str = "oxide://project/local/current.sql";
fn fixture(text: &str) -> LspService<SqlLspServer> {
    let (service, _) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    server.apply_configuration(json!({"oxideAnalysisToken":"warm-private","defaultDialect":"postgres","fileDialects":{(URI):"postgres"},"fileSchemas":{(URI):"00000000-0000-4000-8000-000000000001"},"schemas":[{"id":"00000000-0000-4000-8000-000000000001","database":"app","source_uri":"file:///private-catalog.sql","tables":[{"name":"ledger","columns":[{"name":"secret_value","data_type":"integer","nullable":false}]}],"functions":[{"name":"private_fn","return_type":"integer","parameters":[{"name":"secret_arg","data_type":"integer","optional":false,"mode":"IN"}]}]}]}));
    server.document_manager.update(URI.into(), text.into(), 7);
    service
}
fn request(operation: Value) -> DocumentSemanticParams {
    serde_json::from_value(
        json!({"textDocument":{"uri":URI},"version":7,"dialect":"postgres","operation":operation}),
    )
    .unwrap()
}
fn at(text: &str, needle: &str) -> Position {
    position_for_offset(
        text,
        text.rfind(needle).unwrap() + needle.len() - usize::from(!needle.ends_with('(')),
    )
}
async fn value(server: &SqlLspServer, operation: Value) -> Value {
    server
        .document_semantic_snapshot(request(operation))
        .await
        .unwrap()
        .value
}
#[tokio::test]
async fn warm_catalog_signature_hover_inlays_tokens_and_navigation_are_not_local_sources() {
    let text = "SELECT private_fn(1), secret_value FROM ledger;\nINSERT INTO ledger VALUES(2);";
    let f = fixture(text);
    let server = f.inner();
    let full = server
        .signature_help(SignatureHelpParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: URI.parse().unwrap(),
                },
                position: at(text, "private_fn("),
            },
            work_done_progress_params: Default::default(),
            context: None,
        })
        .await
        .unwrap()
        .unwrap();
    assert!(serde_json::to_string(&full).unwrap().contains("secret_arg"));
    let physical = server
        .inlay_hint(InlayHintParams {
            text_document: TextDocumentIdentifier {
                uri: URI.parse().unwrap(),
            },
            range: Range::new(Position::new(0, 0), position_for_offset(text, text.len())),
            work_done_progress_params: Default::default(),
        })
        .await
        .unwrap()
        .unwrap();
    assert!(!physical.is_empty());
    let operations = vec![
        json!({"kind":"signature","position":at(text,"private_fn(")}),
        json!({"kind":"hover","position":at(text,"secret_value")}),
        json!({"kind":"definitions","position":at(text,"ledger")}),
        json!({"kind":"prepareRename","position":at(text,"ledger")}),
        json!({"kind":"rename","position":at(text,"ledger"),"newName":"new_ledger"}),
        json!({"kind":"tokens"}),
        json!({"kind":"inlayHints","range":{"start":{"line":0,"character":0},"end":position_for_offset(text,text.len())}}),
    ];
    for operation in operations {
        let kind = operation["kind"].as_str().unwrap().to_owned();
        let result = value(server, operation).await;
        assert!(!result.to_string().contains("secret_"), "{kind}: {result}");
        match kind.as_str() {
            "tokens" => assert_eq!(result["data"], json!([])),
            "definitions" | "inlayHints" => assert_eq!(result, json!([])),
            _ => assert!(result.is_null(), "{kind}: {result}"),
        }
    }
    // The warmed source remains available after every local operation.
    assert!(server.get_schema_for_file(URI).unwrap().tables[0].columns[0].name == "secret_value");
}
#[tokio::test]
async fn complex_explicit_local_projection_hover_and_definition_keep_unicode_source() {
    let text="/* 漢😀 */\nWITH base AS (SELECT CAST(1 AS bigint) AS local_value), q AS (SELECT b.* FROM base b) SELECT q.local_value FROM q;";
    let f = fixture(text);
    for dialect in [
        "postgres",
        "mysql",
        "sqlite",
        "sqlserver",
        "hive",
        "clickhouse",
    ] {
        let mut p = request(json!({"kind":"hover","position":at(text,"q.local_value")}));
        p.dialect = dialect.into();
        let result = f.inner().document_semantic_snapshot(p).await.unwrap();
        assert_eq!(result.dialect, dialect);
        assert!(
            result.value.to_string().contains("local_value"),
            "{dialect}: {}",
            result.value
        );
        assert!(!result.value.to_string().contains("secret_value"));
        let mut p = request(json!({"kind":"definitions","position":at(text,"q.local_value")}));
        p.dialect = dialect.into();
        let result = f.inner().document_semantic_snapshot(p).await.unwrap();
        let locations = result.value.as_array().unwrap();
        assert!(!locations.is_empty(), "{dialect}");
        assert!(locations.iter().all(|item| item["uri"] == URI));
    }
}
#[tokio::test]
async fn builtin_signatures_remain_available_without_server_version_or_catalog() {
    let text = "SELECT COALESCE(1, ";
    let f = fixture(text);
    for dialect in [
        "postgres",
        "mysql",
        "sqlite",
        "sqlserver",
        "hive",
        "clickhouse",
    ] {
        let mut p =
            request(json!({"kind":"signature","position":position_for_offset(text,text.len())}));
        p.dialect = dialect.into();
        let result = f.inner().document_semantic_snapshot(p).await.unwrap();
        assert!(
            result.value["signatures"]
                .as_array()
                .is_some_and(|items| !items.is_empty()),
            "{dialect}: {}",
            result.value
        );
        assert_eq!(result.value["activeParameter"], 1);
    }
}
#[tokio::test]
async fn local_ddl_references_and_rename_never_traverse_other_project_documents() {
    let text = "CREATE VIEW local_v AS SELECT 1 AS id; SELECT * FROM local_v;";
    let f = fixture(text);
    let server = f.inner();
    server.document_manager.update(
        "oxide://project/local/other.sql".into(),
        "CREATE VIEW local_v AS SELECT 2; SELECT * FROM local_v;".into(),
        9,
    );
    let position = at(text, "local_v");
    let refs = value(
        server,
        json!({"kind":"references","position":position,"includeDeclaration":true}),
    )
    .await;
    assert_eq!(refs.as_array().unwrap().len(), 2);
    assert!(refs
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["uri"] == URI));
    let refs = value(
        server,
        json!({"kind":"references","position":position,"includeDeclaration":false}),
    )
    .await;
    assert_eq!(refs.as_array().unwrap().len(), 1);
    assert!(
        !value(server, json!({"kind":"prepareRename","position":position}))
            .await
            .is_null()
    );
    let edit = value(
        server,
        json!({"kind":"rename","position":position,"newName":"renamed_v"}),
    )
    .await;
    assert_eq!(edit["changes"].as_object().unwrap().len(), 1);
    assert_eq!(edit["changes"][URI].as_array().unwrap().len(), 2);
    assert!(server
        .document_manager
        .get("oxide://project/local/other.sql")
        .unwrap()
        .contains("local_v"));
}
#[tokio::test]
async fn ambiguous_declarations_and_unsafe_names_do_not_produce_rename_edits() {
    let text = "CREATE VIEW q AS SELECT 1; CREATE VIEW q AS SELECT 2; SELECT * FROM q;";
    let f = fixture(text);
    let pos = at(text, "q");
    assert!(
        value(f.inner(), json!({"kind":"prepareRename","position":pos}))
            .await
            .is_null()
    );
    assert!(value(
        f.inner(),
        json!({"kind":"rename","position":pos,"newName":"q; DROP TABLE ledger"})
    )
    .await
    .is_null());
}
#[tokio::test]
async fn cte_rename_uses_current_document_ranges_only() {
    let text = "WITH q AS (SELECT 1 AS id) SELECT q.id FROM q;";
    let f = fixture(text);
    let position = at(text, "q");
    let edit = value(
        f.inner(),
        json!({"kind":"rename","position":position,"newName":"fresh_q"}),
    )
    .await;
    assert!(
        edit["changes"][URI]
            .as_array()
            .is_some_and(|items| items.len() >= 2),
        "{edit}"
    );
}
#[tokio::test]
async fn local_star_expansion_and_explicit_insert_hints_work_without_physical_columns() {
    let text="WITH q AS (SELECT 1 AS local_id, 'x' AS local_name) SELECT * FROM q;\nINSERT INTO ledger(explicit_value) VALUES(2);";
    let f = fixture(text);
    let pos = at(text, "SELECT *");
    let actions = value(
        f.inner(),
        json!({"kind":"codeActions","range":{"start":pos,"end":pos},"context":{"diagnostics":[]}}),
    )
    .await;
    let encoded = actions.to_string();
    assert!(
        encoded.contains("local_id") && encoded.contains("local_name"),
        "{encoded}; shape={:?}",
        local_schema(text, pos, URI, "postgres")
    );
    assert!(!encoded.contains("secret_value"));
    let hints=value(f.inner(),json!({"kind":"inlayHints","range":{"start":{"line":0,"character":0},"end":position_for_offset(text,text.len())}})).await;
    assert!(hints.to_string().contains("explicit_value"));
    assert!(!hints.to_string().contains("secret_value"));
}
#[tokio::test]
async fn cte_rename_does_not_change_shadowed_names_aliases_or_other_statements() {
    let text="WITH q AS (SELECT 1 AS id) SELECT q.id FROM q WHERE EXISTS (WITH q AS (SELECT 2 AS id) SELECT q.id FROM q); SELECT q.id FROM physical q;";
    let f = fixture(text);
    let position = position_for_offset(text, text.find("FROM q").unwrap() + 5);
    let edit = value(
        f.inner(),
        json!({"kind":"rename","position":position,"newName":"outer_q"}),
    )
    .await;
    let edits = edit["changes"][URI].as_array().unwrap();
    let mut parser = crate::parser::SqlParser::new();
    let tree = parser.parse_syntax_tree(text).unwrap();
    assert_eq!(edits.len(), 3, "{edit}; {}", tree.root_node().to_sexp());
    for edit in edits {
        assert!(
            edit["range"]["start"]["character"].as_u64().unwrap()
                < text.find("WHERE").unwrap() as u64
        );
    }
}
#[tokio::test]
async fn standard_rpc_uses_the_same_nested_cte_binding_without_changing_the_catalog() {
    let text="WITH q AS (SELECT 1 AS id) SELECT q.id FROM q WHERE EXISTS (WITH q AS (SELECT 2 AS id) SELECT q.id FROM q); SELECT q.id FROM ledger q;";
    let f = fixture(text);
    let uri: Url = URI.parse().unwrap();
    let position = position_for_offset(text, text.find("FROM q").unwrap() + 5);
    let renamed = f
        .inner()
        .rename(RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            new_name: "outer_q".into(),
            work_done_progress_params: Default::default(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renamed.changes.unwrap()[&uri].len(), 3);
    let refs = f
        .inner()
        .references(ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            context: ReferenceContext {
                include_declaration: false,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refs.len(), 2);
    assert_eq!(
        f.inner().get_schema_for_file(URI).unwrap().tables[0].columns[0].name,
        "secret_value"
    );
}
#[tokio::test]
async fn every_local_operation_runs_while_analysis_configuration_is_write_locked() {
    let text = "WITH q AS (SELECT 1 AS local_id) SELECT COALESCE(q.local_id, 1) FROM q;";
    let f = fixture(text);
    let server = f.inner();
    let _lock = server.analysis.configuration.write().await;
    let pos = at(text, "q");
    let range = Range::new(Position::new(0, 0), position_for_offset(text, text.len()));
    for operation in [
        json!({"kind":"hover","position":pos}),
        json!({"kind":"definitions","position":pos}),
        json!({"kind":"references","position":pos,"includeDeclaration":true}),
        json!({"kind":"signature","position":at(text,"COALESCE(")}),
        json!({"kind":"prepareRename","position":pos}),
        json!({"kind":"rename","position":pos,"newName":"new_q"}),
        json!({"kind":"codeActions","range":range,"context":{"diagnostics":[]}}),
        json!({"kind":"tokens"}),
        json!({"kind":"inlayHints","range":range}),
    ] {
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            server.document_semantic_snapshot(request(operation)),
        )
        .await
        .expect("local semantics waited for configuration")
        .unwrap();
    }
}
#[tokio::test]
async fn recursive_cte_rename_is_bound_for_all_six_sql_profiles() {
    let text="WITH RECURSIVE q(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM q WHERE id<3) SELECT q.id FROM q;";
    let f = fixture(text);
    for dialect in [
        "postgres",
        "mysql",
        "sqlite",
        "sqlserver",
        "hive",
        "clickhouse",
    ] {
        let dialect_text = if dialect == "sqlserver" {
            text.replace("WITH RECURSIVE", "WITH")
        } else {
            text.into()
        };
        f.inner()
            .document_manager
            .update(URI.into(), dialect_text.clone(), 7);
        let mut p = request(
            json!({"kind":"rename","position":at(&dialect_text,"FROM q"),"newName":"fresh_q"}),
        );
        p.dialect = dialect.into();
        let result = f.inner().document_semantic_snapshot(p).await.unwrap();
        assert_eq!(
            result.value["changes"][URI].as_array().map(Vec::len),
            Some(4),
            "{dialect}: {}",
            result.value
        );
    }
}
#[tokio::test]
async fn quoted_postgres_cte_identity_does_not_fold_into_a_distinct_lowercase_binding() {
    let text="/* 漢😀 */ WITH \"Q\" AS (SELECT 1 AS id), q AS (SELECT 2 AS id) SELECT \"Q\".id, q.id FROM \"Q\", q;";
    let f = fixture(text);
    let pos = position_for_offset(text, text.rfind("FROM \"Q\"").unwrap() + 6);
    let result = value(
        f.inner(),
        json!({"kind":"rename","position":pos,"newName":"fresh_q"}),
    )
    .await;
    assert_eq!(
        result["changes"][URI].as_array().map(Vec::len),
        Some(3),
        "{result}"
    );
}
#[tokio::test]
async fn stale_missing_and_unregistered_requests_are_rejected() {
    let f = fixture("SELECT 1");
    let mut p = request(json!({"kind":"tokens"}));
    p.version = 6;
    assert_eq!(
        f.inner()
            .document_semantic_snapshot(p)
            .await
            .unwrap_err()
            .code,
        ErrorCode::ServerError(-32801)
    );
    let mut p = request(json!({"kind":"tokens"}));
    p.dialect = "unknown".into();
    assert_eq!(
        f.inner()
            .document_semantic_snapshot(p)
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidParams
    );
    let mut p = request(json!({"kind":"tokens"}));
    p.text_document.uri = "oxide://project/local/missing.sql".parse().unwrap();
    assert_eq!(
        f.inner()
            .document_semantic_snapshot(p)
            .await
            .unwrap_err()
            .code,
        ErrorCode::ServerError(-32801)
    );
    let p = request(
        json!({"kind":"inlayHints","range":{"start":{"line":2,"character":0},"end":{"line":0,"character":0}}}),
    );
    assert_eq!(
        f.inner()
            .document_semantic_snapshot(p)
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidParams
    );
}
#[test]
fn operation_deserialization_rejects_metadata_authority_and_invalid_variants() {
    let base = json!({"textDocument":{"uri":URI},"version":7,"dialect":"postgres","operation":{"kind":"tokens"}});
    for field in ["oxideAnalysisToken", "schema", "metadataAllowed"] {
        let mut p = base.clone();
        p[field] = json!("private");
        assert!(
            serde_json::from_value::<DocumentSemanticParams>(p.clone()).is_err(),
            "{p}"
        );
    }
    for operation in [
        json!({"kind":"hover"}),
        json!({"kind":"tokens","position":{"line":0,"character":0}}),
        json!({"kind":"signature","position":{"line":-1,"character":0}}),
        json!({"kind":"signature","position":{"line":0,"character":0},"schema":"private"}),
        json!({"kind":"unknown"}),
    ] {
        let mut p = base.clone();
        p["operation"] = operation;
        assert!(serde_json::from_value::<DocumentSemanticParams>(p).is_err());
    }
}
