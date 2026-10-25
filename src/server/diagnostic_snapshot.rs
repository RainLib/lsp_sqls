//! Coherent diagnostic readback; syntax mode has no schema/catalog access.
use super::{diagnostics::document_diagnostics_with_metadata, runtime::SqlLspServer};
use tower_lsp::{
    jsonrpc::{Error, ErrorCode, Result},
    lsp_types::{Diagnostic, TextDocumentIdentifier, Url},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSnapshotMode {
    Syntax,
    Metadata,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSnapshotParams {
    pub text_document: TextDocumentIdentifier,
    pub version: i32,
    pub dialect: String,
    pub mode: DiagnosticSnapshotMode,
    pub oxide_analysis_token: Option<String>,
}
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSnapshot {
    pub uri: Url,
    pub version: i32,
    pub dialect: String,
    pub mode: DiagnosticSnapshotMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oxide_analysis_token: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
}
fn modified() -> Error {
    Error {
        code: ErrorCode::ServerError(-32801),
        message: "Document or diagnostic context changed".into(),
        data: None,
    }
}

impl SqlLspServer {
    pub async fn document_diagnostic_snapshot(
        &self,
        params: DiagnosticSnapshotParams,
    ) -> Result<DiagnosticSnapshot> {
        if params.version < 0 {
            return Err(Error::invalid_params("Invalid document version"));
        }
        let uri = params.text_document.uri.to_string();
        let profile = self
            .dialect_registry
            .get_explicit_profile(&params.dialect)
            .ok_or_else(|| Error::invalid_params("Unknown diagnostic profile"))?;
        let configuration = if params.mode == DiagnosticSnapshotMode::Metadata {
            Some(self.analysis.configuration.read().await)
        } else {
            None
        };
        let document = self.document_manager.snapshot(&uri).ok_or_else(modified)?;
        if document.version != params.version {
            return Err(modified());
        }
        let token = if params.mode == DiagnosticSnapshotMode::Metadata {
            let expected = params
                .oxide_analysis_token
                .as_ref()
                .map(|value| serde_json::Value::String(value.clone()));
            let expected = super::analysis_provenance::parse_analysis_token(expected.as_ref())
                .ok_or_else(|| {
                    Error::invalid_params("Metadata diagnostics require a configuration token")
                })?;
            let actual = self
                .analysis
                .provenance
                .read()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            if actual.as_deref() != Some(&expected)
                || self
                    .get_dialect_for_file(&uri)
                    .is_none_or(|actual| !std::sync::Arc::ptr_eq(&actual, &profile))
            {
                return Err(modified());
            }
            actual
        } else {
            None
        };
        let diagnostics = if params.mode == DiagnosticSnapshotMode::Metadata {
            let schema = self.get_schema_for_file(&uri);
            document_diagnostics_with_metadata(
                &*profile,
                &document.text,
                schema.as_ref(),
                Some(&self.schema_manager),
            )
            .await
        } else {
            document_diagnostics_with_metadata(&*profile, &document.text, None, None).await
        };
        if self.document_manager.revision(&uri) != Some(document.revision)
            || self
                .dialect_registry
                .get_by_name(&params.dialect)
                .is_none_or(|current| !std::sync::Arc::ptr_eq(&current, &profile))
        {
            return Err(modified());
        }
        drop(configuration);
        Ok(DiagnosticSnapshot {
            uri: params.text_document.uri,
            version: document.version,
            dialect: params.dialect,
            mode: params.mode,
            oxide_analysis_token: token,
            diagnostics,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tower_lsp::LspService;
    const URI: &str = "oxide://snapshot/query.sql";
    #[tokio::test]
    async fn duckdb_preserves_shared_ambiguity_and_mutation_inspections() {
        let (service, _) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        seed(server, "WITH q AS (SELECT l.* FROM ledger l) SELECT id FROM q a JOIN ledger b ON 1=1; DELETE FROM ledger;");
        server.apply_configuration(json!({"oxideAnalysisToken":"current","defaultDialect":"duckdb","fileDialects":{(URI):"duckdb"}}));
        let mut p = request(DiagnosticSnapshotMode::Metadata);
        p.dialect = "duckdb".into();
        let result = server.document_diagnostic_snapshot(p).await.unwrap();
        for code in ["OXIDE004", "OXIDE001"] {
            assert!(
                result.diagnostics.iter().any(|item| item.code
                    == Some(tower_lsp::lsp_types::NumberOrString::String(code.into()))),
                "{code}: {:?}",
                result.diagnostics
            );
        }
    }
    #[tokio::test]
    async fn snapshots_echo_reviewed_aliases_and_preserve_exact_adapter_authority() {
        let (service, _) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        seed(server, "SELECT (1 + );");
        for dialect in ["duckdb", "mariadb", "postgresql", "mssql", "argo", "turso"] {
            let mut p = request(DiagnosticSnapshotMode::Syntax);
            p.dialect = dialect.into();
            let result = server.document_diagnostic_snapshot(p).await.unwrap();
            assert_eq!(result.dialect, dialect);
            assert!(result.oxide_analysis_token.is_none());
            assert!(!result.diagnostics.is_empty(), "{dialect}");
        }
        server.apply_configuration(json!({"oxideAnalysisToken":"current","defaultDialect":"sqlite","fileDialects":{(URI):"sqlite"}}));
        let mut wrong = request(DiagnosticSnapshotMode::Metadata);
        wrong.dialect = "duckdb".into();
        assert!(
            server.document_diagnostic_snapshot(wrong).await.is_err(),
            "different adapters with the same internal name must not share metadata authority"
        );
        server.apply_configuration(json!({"oxideAnalysisToken":"current","defaultDialect":"duckdb","fileDialects":{(URI):"duckdb"}}));
        let mut right = request(DiagnosticSnapshotMode::Metadata);
        right.dialect = "duckdb".into();
        assert_eq!(
            server
                .document_diagnostic_snapshot(right)
                .await
                .unwrap()
                .dialect,
            "duckdb"
        );
    }
    fn seed(server: &SqlLspServer, sql: &str) {
        server.apply_configuration(json!({"oxideAnalysisToken":"current","defaultDialect":"postgres","fileDialects":{(URI):"postgres"},
            "fileSchemas":{(URI):"00000000-0000-4000-8000-000000000001"},"schemas":[{"id":"00000000-0000-4000-8000-000000000001","database":"app","tables":[
                {"name":"ledger","columns":[{"name":"id","data_type":"integer","nullable":false}]}],"functions":[]}]}));
        server.document_manager.update(URI.into(), sql.into(), 7);
    }
    fn request(mode: DiagnosticSnapshotMode) -> DiagnosticSnapshotParams {
        DiagnosticSnapshotParams {
            text_document: TextDocumentIdentifier {
                uri: URI.parse().unwrap(),
            },
            version: 7,
            dialect: "postgres".into(),
            mode,
            oxide_analysis_token: Some("current".into()),
        }
    }
    #[tokio::test]
    async fn syntax_mode_never_borrows_schema_fields_or_a_configuration_grant() {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        seed(
            server,
            "WITH q AS (SELECT l.* FROM ledger l) SELECT id FROM q a JOIN ledger b ON 1=1;",
        );
        let full = server
            .document_diagnostic_snapshot(request(DiagnosticSnapshotMode::Metadata))
            .await
            .unwrap();
        assert!(full.diagnostics.iter().any(|item| item.code
            == Some(tower_lsp::lsp_types::NumberOrString::String(
                "OXIDE004".into()
            ))));
        let syntax = server
            .document_diagnostic_snapshot(request(DiagnosticSnapshotMode::Syntax))
            .await
            .unwrap();
        assert!(syntax.oxide_analysis_token.is_none());
        assert!(syntax.diagnostics.iter().all(|item| item.code
            != Some(tower_lsp::lsp_types::NumberOrString::String(
                "OXIDE004".into()
            ))));
    }
    #[tokio::test]
    async fn syntax_mode_reports_parser_errors_without_any_schema_or_token() {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        server
            .document_manager
            .update(URI.into(), "SELECT (1 + );".into(), 7);
        let result = server
            .document_diagnostic_snapshot(request(DiagnosticSnapshotMode::Syntax))
            .await
            .unwrap();
        assert_eq!(result.version, 7);
        assert!(result.oxide_analysis_token.is_none());
        assert!(
            result.diagnostics.iter().any(|item| item.code
                == Some(tower_lsp::lsp_types::NumberOrString::String(
                    "SYNTAX_ERROR".into()
                ))),
            "{:?}",
            result.diagnostics
        );
    }
    #[tokio::test]
    async fn syntax_uses_the_requested_profile_instead_of_stale_file_routing() {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        seed(
            server,
            "SELECT row_number() OVER(ORDER BY id) AS rn FROM ledger QUALIFY rn=1;",
        );
        for (profile, unsupported) in [("hive", true), ("clickhouse", false)] {
            let mut params = request(DiagnosticSnapshotMode::Syntax);
            params.dialect = profile.into();
            let result = server.document_diagnostic_snapshot(params).await.unwrap();
            assert_eq!(result.dialect, profile);
            assert_eq!(
                result.diagnostics.iter().any(|item| item.code
                    == Some(tower_lsp::lsp_types::NumberOrString::String(
                        "UNSUPPORTED_DIALECT_SYNTAX".into()
                    ))),
                unsupported
            );
        }
        let mut metadata = request(DiagnosticSnapshotMode::Metadata);
        metadata.dialect = "hive".into();
        assert!(server.document_diagnostic_snapshot(metadata).await.is_err());
    }
    #[tokio::test]
    async fn metadata_requires_the_exact_current_token_and_version() {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        seed(server, "SELECT 1;");
        let mut wrong = request(DiagnosticSnapshotMode::Metadata);
        wrong.oxide_analysis_token = Some("old".into());
        assert_eq!(
            server
                .document_diagnostic_snapshot(wrong)
                .await
                .unwrap_err()
                .code,
            ErrorCode::ServerError(-32801)
        );
        let mut missing = request(DiagnosticSnapshotMode::Metadata);
        missing.oxide_analysis_token = None;
        assert!(server.document_diagnostic_snapshot(missing).await.is_err());
        let mut version = request(DiagnosticSnapshotMode::Syntax);
        version.version = 6;
        assert_eq!(
            server
                .document_diagnostic_snapshot(version)
                .await
                .unwrap_err()
                .code,
            ErrorCode::ServerError(-32801)
        );
    }
    #[tokio::test]
    async fn closed_documents_and_unknown_profiles_cannot_return_empty_success() {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        assert!(server
            .document_diagnostic_snapshot(request(DiagnosticSnapshotMode::Syntax))
            .await
            .is_err());
        seed(server, "SELECT 1");
        let mut unknown = request(DiagnosticSnapshotMode::Syntax);
        unknown.dialect = "unregistered".into();
        assert!(server.document_diagnostic_snapshot(unknown).await.is_err());
    }
}
