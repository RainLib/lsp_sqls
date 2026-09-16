//! Inline types for the SQL language server.
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use tower_lsp::lsp_types::*;
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineCompletionCandidate {
    pub(super) label: String,
    pub(super) insert_text: Option<String>,
    pub(super) kind: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineCompletionDiagnostic {
    pub(super) severity: Option<u32>,
    pub(super) message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineCompletionContextResponse {
    pub(super) dialect: String,
    pub(super) clause: String,
    pub(super) statement_range: Option<Range>,
    pub(super) ctes: Vec<String>,
    pub(super) referenced_objects: Vec<String>,
    pub(super) aliases: HashMap<String, String>,
    pub(super) expected_kinds: Vec<String>,
    pub(super) candidates: Vec<InlineCompletionCandidate>,
    pub(super) diagnostics: Vec<InlineCompletionDiagnostic>,
    pub(super) error_count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineCompletionValidationParams {
    pub(super) text_document: TextDocumentIdentifier,
    pub(super) text: String,
}
