//! Revision-fenced, ordered diagnostic publications.
use super::analysis_provenance::{ContextDiagnostics, ContextDiagnosticsParams};
use super::{diagnostics::document_diagnostics_with_metadata, runtime::SqlLspServer};
use tower_lsp::lsp_types::Url;
impl SqlLspServer {
    pub(super) async fn publish_diagnostics_for_open_documents(&self) {
        for uri in self.document_manager.uris() {
            self.publish_document_diagnostics(&uri).await;
        }
    }

    pub(super) async fn publish_document_diagnostics(&self, uri: &str) -> bool {
        let Ok(parsed_uri) = Url::parse(uri) else {
            return false;
        };
        let (revision, token, document, diagnostics) = {
            let revision = self.analysis.configuration.read().await;
            let Some(document) = self.document_manager.snapshot(uri) else {
                return false;
            };
            let Some(dialect) = self.get_dialect_for_file(uri) else {
                return false;
            };
            let schema = self.get_schema_for_file(uri);
            let diagnostics = document_diagnostics_with_metadata(
                &*dialect,
                &document.text,
                schema.as_ref(),
                Some(&self.schema_manager),
            )
            .await;
            let token = self
                .analysis
                .provenance
                .read()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            (*revision, token, document, diagnostics)
        };
        // Serialize sends, not analysis. Recheck after waiting for the sender:
        // an older calculation must never overtake a newer publication.
        let _publication = self.analysis.publication.lock().await;
        {
            let current = self.analysis.configuration.read().await;
            if *current != revision
                || self.document_manager.revision(uri) != Some(document.revision)
            {
                return false;
            }
        }
        // No configuration guard across bounded client I/O/backpressure.
        self.client
            .send_notification::<ContextDiagnostics>(ContextDiagnosticsParams {
                diagnostics: tower_lsp::lsp_types::PublishDiagnosticsParams {
                    uri: parsed_uri,
                    diagnostics,
                    version: Some(document.version),
                },
                oxide_analysis_token: token,
            })
            .await;
        true
    }
}
