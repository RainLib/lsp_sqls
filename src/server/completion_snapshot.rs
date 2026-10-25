//! Catalog completion with version, explicit profile and committed provenance.
use super::{completion_constraints::CompletionKeywordConstraints, runtime::SqlLspServer};
use std::sync::Arc;
use tower_lsp::{
    jsonrpc::{Error, Result},
    lsp_types::{CompletionContext, CompletionItem, Position, TextDocumentIdentifier, Url},
};

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionSnapshotParams {
    pub text_document: TextDocumentIdentifier,
    pub version: i32,
    pub dialect: String,
    pub position: Position,
    pub oxide_analysis_token: String,
    pub context: Option<CompletionContext>,
}
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionSnapshot {
    pub uri: Url,
    pub version: i32,
    pub dialect: String,
    pub position: Position,
    pub oxide_analysis_token: String,
    pub items: Vec<CompletionItem>,
    pub is_incomplete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_continuation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern_escape: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraints: Option<CompletionKeywordConstraints>,
}
fn modified() -> Error {
    Error {
        code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32801),
        message: "Completion snapshot changed".into(),
        data: None,
    }
}
fn profile_identity(profile: &str) -> &str {
    match profile {
        "postgresql" | "pgsql" | "psql" => "postgres",
        "mssql" | "tsql" => "sqlserver",
        "sqlite3" => "sqlite",
        "hql" => "hive",
        "ch" => "clickhouse",
        _ => profile,
    }
}
impl SqlLspServer {
    pub async fn completion_snapshot(
        &self,
        params: CompletionSnapshotParams,
    ) -> Result<CompletionSnapshot> {
        let token = super::analysis_provenance::parse_analysis_token(Some(
            &serde_json::Value::String(params.oxide_analysis_token),
        ))
        .ok_or_else(|| Error::invalid_params("Invalid configuration token"))?;
        let _configuration = self.analysis.configuration.read().await;
        if params.version < 0
            || self
                .analysis
                .provenance
                .read()
                .unwrap_or_else(|error| error.into_inner())
                .as_deref()
                != Some(token.as_str())
        {
            return Err(modified());
        }
        let requested = self
            .dialect_registry
            .get_explicit_profile(&params.dialect)
            .ok_or_else(|| Error::invalid_params("Unknown completion profile"))?;
        let uri = params.text_document.uri;
        let identity = self
            .dialect_identity_for_file(uri.as_str())
            .ok_or_else(modified)?;
        let actual = self
            .dialect_registry
            .get_explicit_profile(&identity)
            .ok_or_else(modified)?;
        if profile_identity(&params.dialect) != profile_identity(&identity)
            || !Arc::ptr_eq(&requested, &actual)
        {
            return Err(modified());
        }
        let document = self
            .document_manager
            .snapshot(uri.as_str())
            .ok_or_else(modified)?;
        if document.version != params.version {
            return Err(modified());
        }
        if super::completion_context::checked_position_to_byte_offset(
            &document.text,
            params.position,
        )
        .is_none()
        {
            return Err(Error::invalid_params("Invalid completion position"));
        }
        let mut candidates = self
            .completion_candidates_at(
                uri.as_str(),
                &document.text,
                params.position,
                params.context.as_ref(),
            )
            .await
            .ok_or_else(modified)?;
        candidates.is_incomplete |= candidates.items.len() > 512;
        candidates.items.truncate(512);
        let mut bytes = 2 * 1024 * 1024;
        candidates.items.retain(|item| {
            let size = serde_json::to_vec(item).map_or(usize::MAX, |value| value.len());
            if size > bytes {
                candidates.is_incomplete = true;
                false
            } else {
                bytes -= size;
                true
            }
        });
        if self.document_manager.revision(uri.as_str()) != Some(document.revision) {
            return Err(modified());
        }
        Ok(CompletionSnapshot {
            uri,
            version: params.version,
            dialect: params.dialect,
            position: params.position,
            oxide_analysis_token: token,
            items: candidates.items,
            is_incomplete: candidates.is_incomplete,
            constraints: candidates.constraints,
            value_type: candidates.value_type,
            value_role: candidates.value_role,
            logical_continuation: candidates.logical_continuation,
            pattern_escape: candidates.pattern_escape,
        })
    }
}

#[cfg(test)]
mod tests;
