//! Configuration commits and analysis/publication ordering.
use super::configuration_patch::ConfigurationPatch;
use super::runtime::SqlLspServer;
use tokio::sync::{Mutex, RwLock};
use tower_lsp::lsp_types::MessageType;

/// Readers may analyze concurrently. A configuration writer commits schemas,
/// routing and preferences together; no client I/O runs under this gate.
#[derive(Default)]
pub(super) struct AnalysisState {
    pub(super) configuration: RwLock<u64>,
    pub(super) publication: Mutex<()>,
    pub(super) provenance: std::sync::RwLock<Option<String>>,
}

impl SqlLspServer {
    pub(super) fn apply_configuration(
        &self,
        settings: serde_json::Value,
    ) -> Vec<(MessageType, String)> {
        // Every attempted replacement revokes the previous receipt, including
        // non-object payloads. Rejected patches retain the last valid settings
        // but cannot authorize them under either the old or proposed token.
        *self
            .analysis
            .provenance
            .write()
            .unwrap_or_else(|error| error.into_inner()) = None;
        let patch = match ConfigurationPatch::parse(settings, &self.dialect_registry) {
            Ok(patch) => patch,
            Err(message) => return vec![(MessageType::WARNING, message.to_owned())],
        };
        let mut messages = Vec::new();
        if let Some(default_dialect) = patch.default_dialect {
            *self
                .default_dialect
                .write()
                .unwrap_or_else(|error| error.into_inner()) = default_dialect;
            messages.push((MessageType::INFO, "Updated default dialect".into()));
        }
        if let Some(preferences) = patch.completion_preferences {
            *self
                .completion_preferences
                .write()
                .unwrap_or_else(|error| error.into_inner()) = preferences;
        }
        if let Some(preferences) = patch.formatting_preferences {
            *self
                .formatting_preferences
                .write()
                .unwrap_or_else(|error| error.into_inner()) = preferences;
        }
        if let Some(schemas) = patch.schemas {
            let count = schemas.len();
            self.schema_manager.clear();
            for schema in schemas {
                self.schema_manager.register(schema);
            }
            self.inferred_file_schemas.clear();
            messages.push((MessageType::INFO, format!("Updated {count} schemas")));
        }
        if let Some(routes) = patch.file_schemas {
            self.configured_file_schemas.clear();
            self.inferred_file_schemas.clear();
            for (uri, id) in routes {
                self.configured_file_schemas.insert(uri, id);
            }
            messages.push((MessageType::INFO, "Updated file-schema mappings".into()));
        }
        if let Some(routes) = patch.file_dialects {
            self.configured_file_dialects.clear();
            for (uri, dialect) in routes {
                self.configured_file_dialects.insert(uri, dialect);
            }
            messages.push((MessageType::INFO, "Updated file-dialect mappings".into()));
        }
        self.inferred_file_dialects.clear();
        self.ensure_dialects_for_open_documents();
        // Publish provenance last, while the caller still owns the exclusive
        // configuration gate. A receipt observes the complete validated commit.
        *self
            .analysis
            .provenance
            .write()
            .unwrap_or_else(|error| error.into_inner()) = patch.oxide_analysis_token;
        messages
    }
}
