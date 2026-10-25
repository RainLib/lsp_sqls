//! Document structure without configuration, catalog or other-document reads.
use super::document_structure::{document_symbols, folding_ranges, selection_range_for_position};
use super::project_index::project_sql_symbol_occurrences;
use super::runtime::SqlLspServer;
use std::sync::Arc;
use tower_lsp::{
    jsonrpc::{Error, ErrorCode, Result},
    lsp_types::{Position, TextDocumentIdentifier, Url},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentStructureKind {
    Symbols,
    Folding,
    Selection,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentStructureParams {
    pub text_document: TextDocumentIdentifier,
    pub version: i32,
    pub dialect: String,
    pub kind: DocumentStructureKind,
    #[serde(default)]
    pub positions: Vec<Position>,
}
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentStructureSnapshot {
    pub uri: Url,
    pub version: i32,
    pub dialect: String,
    pub kind: DocumentStructureKind,
    pub items: serde_json::Value,
}
fn modified() -> Error {
    Error {
        code: ErrorCode::ServerError(-32801),
        message: "Document or structure profile changed".into(),
        data: None,
    }
}
impl SqlLspServer {
    pub async fn document_structure_snapshot(
        &self,
        params: DocumentStructureParams,
    ) -> Result<DocumentStructureSnapshot> {
        if params.version < 0
            || params.kind != DocumentStructureKind::Selection && !params.positions.is_empty()
        {
            return Err(Error::invalid_params("Invalid document structure request"));
        }
        let profile = self
            .dialect_registry
            .get_explicit_profile(&params.dialect)
            .ok_or_else(|| Error::invalid_params("Unknown structure profile"))?;
        let uri = params.text_document.uri.to_string();
        let document = self.document_manager.snapshot(&uri).ok_or_else(modified)?;
        if document.version != params.version {
            return Err(modified());
        }
        // Do not call get_schema_for_file or project_sql_index_for_document:
        // only this immutable SQL snapshot can contribute structure symbols.
        let items = match params.kind {
            DocumentStructureKind::Symbols => serde_json::to_value(document_symbols(
                &document.text,
                None,
                &project_sql_symbol_occurrences(&document.text),
            )),
            DocumentStructureKind::Folding => serde_json::to_value(folding_ranges(&document.text)),
            DocumentStructureKind::Selection => serde_json::to_value(
                params
                    .positions
                    .iter()
                    .map(|position| {
                        selection_range_for_position(&document.text, *position, profile.name())
                    })
                    .collect::<Vec<_>>(),
            ),
        }
        .map_err(|_| Error::internal_error())?;
        if self.document_manager.revision(&uri) != Some(document.revision)
            || self
                .dialect_registry
                .get_by_name(&params.dialect)
                .is_none_or(|current| !Arc::ptr_eq(&current, &profile))
        {
            return Err(modified());
        }
        Ok(DocumentStructureSnapshot {
            uri: params.text_document.uri,
            version: document.version,
            dialect: params.dialect,
            kind: params.kind,
            items,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::{
        lsp_types::{DocumentSymbolParams, DocumentSymbolResponse},
        LanguageServer, LspService,
    };
    const URI: &str = "oxide://project/structure/current.sql";
    #[tokio::test]
    async fn reviewed_aliases_keep_their_wire_identity_for_local_structure() {
        let service = fixture("WITH q AS (SELECT 1 AS id) SELECT q.id FROM q;");
        for dialect in ["duckdb", "mariadb", "postgresql", "mssql", "argo", "turso"] {
            let mut p = request(DocumentStructureKind::Symbols);
            p.dialect = dialect.into();
            let result = service
                .inner()
                .document_structure_snapshot(p)
                .await
                .unwrap();
            assert_eq!(result.dialect, dialect);
            assert!(result
                .items
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["name"] == "q"));
        }
    }
    fn fixture(text: &str) -> LspService<SqlLspServer> {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        server.apply_configuration(serde_json::json!({"oxideAnalysisToken":"secret-context","defaultDialect":"postgres","fileSchemas":{(URI):"00000000-0000-4000-8000-000000000001"},"schemas":[{"id":"00000000-0000-4000-8000-000000000001","database":"app","tables":[{"name":"ledger","columns":[{"name":"id","data_type":"integer","nullable":false}]}],"functions":[{"name":"private_fn","return_type":"integer","parameters":[{"name":"secret_arg","data_type":"integer","optional":false,"mode":"IN"}]}]}]}));
        server.document_manager.update(URI.into(), text.into(), 7);
        service
    }
    fn request(kind: DocumentStructureKind) -> DocumentStructureParams {
        DocumentStructureParams {
            text_document: TextDocumentIdentifier {
                uri: URI.parse().unwrap(),
            },
            version: 7,
            dialect: "postgres".into(),
            kind,
            positions: Vec::new(),
        }
    }
    #[tokio::test]
    async fn document_symbols_exclude_a_warm_physical_catalog_and_routine_signatures() {
        let service=fixture("CREATE VIEW local_v AS SELECT 1 AS id;\nWITH q AS (SELECT * FROM ledger) SELECT private_fn(q.id) FROM q;");
        let server = service.inner();
        let full = server
            .document_symbol(DocumentSymbolParams {
                text_document: TextDocumentIdentifier {
                    uri: URI.parse().unwrap(),
                },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            })
            .await
            .unwrap()
            .unwrap();
        let DocumentSymbolResponse::Nested(full) = full else {
            panic!("nested outline expected")
        };
        assert!(full.iter().any(|item| item.name == "ledger"));
        assert!(full.iter().any(|item| item.name == "private_fn"
            && item
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("secret_arg")));
        let local = server
            .document_structure_snapshot(request(DocumentStructureKind::Symbols))
            .await
            .unwrap();
        let names = local
            .items
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(names.contains(&"local_v"));
        assert!(names.contains(&"q"));
        assert!(!names.contains(&"ledger"));
        assert!(!names.contains(&"private_fn"));
        assert!(!local.items.to_string().contains("secret_arg"));
        assert_eq!(local.version, 7);
        assert_eq!(local.dialect, "postgres");
    }
    #[tokio::test]
    async fn structure_does_not_wait_for_or_read_the_configuration_transaction() {
        let service = fixture("WITH q AS (\n SELECT 1 AS id\n)\nSELECT id FROM q;");
        let server = service.inner();
        let _configuration = server.analysis.configuration.write().await;
        for kind in [
            DocumentStructureKind::Symbols,
            DocumentStructureKind::Folding,
            DocumentStructureKind::Selection,
        ] {
            let mut params = request(kind);
            if kind == DocumentStructureKind::Selection {
                params.positions.push(Position::new(1, 9));
            }
            let response = tokio::time::timeout(
                std::time::Duration::from_millis(100),
                server.document_structure_snapshot(params),
            )
            .await
            .expect("schema-free structure must not lock configuration")
            .unwrap();
            assert!(!response.items.as_array().unwrap().is_empty());
        }
    }
    #[tokio::test]
    async fn structure_uses_the_requested_profile_and_only_current_document_definitions() {
        let sql="/* 漢😀 */\nWITH a AS (\n SELECT 1 AS id\n), q AS (\n SELECT id FROM a\n)\nSELECT q.id FROM q;";
        let service = fixture(sql);
        let server = service.inner();
        server.document_manager.update(
            "oxide://project/structure/other.sql".into(),
            "CREATE VIEW foreign_v AS SELECT 1;".into(),
            1,
        );
        for dialect in [
            "postgres",
            "mysql",
            "sqlite",
            "sqlserver",
            "hive",
            "clickhouse",
        ] {
            let mut params = request(DocumentStructureKind::Symbols);
            params.dialect = dialect.into();
            let response = server.document_structure_snapshot(params).await.unwrap();
            assert_eq!(response.dialect, dialect);
            let names = response
                .items
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["name"].as_str().unwrap())
                .collect::<Vec<_>>();
            assert!(names.contains(&"a") && names.contains(&"q"));
            assert!(!names.contains(&"foreign_v"));
            let mut params = request(DocumentStructureKind::Selection);
            params.dialect = dialect.into();
            params.positions = vec![Position::new(2, 10), Position::new(6, 8)];
            let response = server.document_structure_snapshot(params).await.unwrap();
            assert_eq!(response.items.as_array().unwrap().len(), 2);
        }
    }
    #[tokio::test]
    async fn stale_wire_versions_and_closed_documents_cannot_supply_structure() {
        let service = fixture("WITH q AS (SELECT 1) SELECT * FROM q");
        let server = service.inner();
        server
            .document_manager
            .update(URI.into(), "SELECT 2".into(), 8);
        assert!(server
            .document_structure_snapshot(request(DocumentStructureKind::Symbols))
            .await
            .is_err());
        let mut params = request(DocumentStructureKind::Folding);
        params.text_document.uri = "oxide://project/structure/missing.sql".parse().unwrap();
        assert!(server.document_structure_snapshot(params).await.is_err());
    }
    #[tokio::test]
    async fn invalid_profiles_versions_and_wrong_operation_arguments_are_rejected() {
        let service = fixture("SELECT 1");
        let server = service.inner();
        for dialect in ["POSTGRES", "unknown", "postgres "] {
            let mut params = request(DocumentStructureKind::Symbols);
            params.dialect = dialect.into();
            assert!(server.document_structure_snapshot(params).await.is_err());
        }
        let mut params = request(DocumentStructureKind::Symbols);
        params.version = -1;
        assert!(server.document_structure_snapshot(params).await.is_err());
        let mut params = request(DocumentStructureKind::Folding);
        params.positions.push(Position::new(0, 0));
        assert!(server.document_structure_snapshot(params).await.is_err());
    }
    #[test]
    fn request_deserialization_rejects_authority_fields_and_unknown_operations() {
        let valid = serde_json::json!({"textDocument":{"uri":URI},"version":7,"dialect":"postgres","kind":"symbols"});
        assert!(serde_json::from_value::<DocumentStructureParams>(valid.clone()).is_ok());
        for field in ["oxideAnalysisToken", "schema", "schemas"] {
            let mut invalid = valid.clone();
            invalid[field] = serde_json::json!("do-not-read");
            assert!(serde_json::from_value::<DocumentStructureParams>(invalid).is_err());
        }
        let mut invalid = valid;
        invalid["kind"] = serde_json::json!("workspace");
        assert!(serde_json::from_value::<DocumentStructureParams>(invalid).is_err());
    }
}
