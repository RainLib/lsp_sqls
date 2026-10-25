//! SQL LSP API. Transport, document state, and semantic features have separate owners.
mod analysis_configuration;
mod analysis_provenance;
mod analysis_publication;
mod analysis_state;
mod auxiliary_documents;
mod builtin_completion;
mod call_completion;
mod code_actions;
mod comparison_value_types;
mod completion_case;
mod completion_constraints;
mod completion_context;
mod completion_identifiers;
mod completion_quotes;
mod completion_regions;
mod completion_resolve;
mod completion_snapshot;
mod configuration_patch;
mod diagnostic_snapshot;
mod diagnostics;
mod dialect_resolution;
mod document_completions;
mod document_cte_locations;
mod document_semantic_actions;
mod document_semantics;
mod document_structure;
mod document_structure_snapshot;
mod documents;
mod execute_completion;
mod execute_context;
mod inline_types;
mod insert_completion;
mod insert_hints;
mod is_predicate_completion;
mod join_actions;
mod limits;
mod local_relations;
mod locations;
mod logical_continuations;
mod named_completion;
mod preferences;
mod project_index;
mod project_types;
mod protocol;
mod routine_arguments;
mod routine_context;
mod routine_hints;
mod routine_identity;
mod routine_overloads;
mod routine_parameters;
mod runtime;
mod schema_resolution;
mod semantic_tokens;
mod signature_help;
mod sql_scan;
mod tsql_module_parameters;
mod tsql_module_scope;
mod tsql_types;
mod tsql_variable_completion;
mod tsql_variables;
pub use analysis_configuration::{AnalysisConfigurationParams, AnalysisConfigurationReceipt};
pub use completion_constraints::CompletionKeywordConstraints;
pub use completion_snapshot::{CompletionSnapshot, CompletionSnapshotParams};
pub use diagnostic_snapshot::{
    DiagnosticSnapshot, DiagnosticSnapshotMode, DiagnosticSnapshotParams,
};
pub use document_completions::{DocumentCompletionParams, DocumentCompletionSnapshot};
pub use document_semantics::{
    DocumentSemanticOperation, DocumentSemanticParams, DocumentSemanticSnapshot,
};
pub use document_structure_snapshot::{
    DocumentStructureKind, DocumentStructureParams, DocumentStructureSnapshot,
};
pub use inline_types::{
    InlineCompletionCandidate, InlineCompletionContextResponse, InlineCompletionDiagnostic,
    InlineCompletionValidationParams,
};
pub use runtime::SqlLspServer;
#[cfg(test)]
mod tests;
