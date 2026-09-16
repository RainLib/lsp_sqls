//! SQL LSP API. Transport, document state, and semantic features have separate owners.
mod builtin_completion;
mod code_actions;
mod completion_case;
mod completion_context;
mod completion_identifiers;
mod completion_resolve;
mod diagnostics;
mod dialect_resolution;
mod document_structure;
mod documents;
mod inline_types;
mod insert_completion;
mod insert_hints;
mod join_actions;
mod limits;
mod local_relations;
mod locations;
mod preferences;
mod project_index;
mod project_types;
mod protocol;
mod routine_hints;
mod runtime;
mod schema_resolution;
mod semantic_tokens;
mod signature_help;
mod sql_scan;
pub use inline_types::{
    InlineCompletionCandidate, InlineCompletionContextResponse, InlineCompletionDiagnostic,
    InlineCompletionValidationParams,
};
pub use runtime::SqlLspServer;
#[cfg(test)]
mod tests;
