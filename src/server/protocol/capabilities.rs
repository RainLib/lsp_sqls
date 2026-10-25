use super::super::completion_resolve::client_supports_completion_documentation_resolve;
use super::super::semantic_tokens::semantic_tokens_legend;
use std::sync::atomic::{AtomicBool, Ordering};
use tower_lsp::lsp_types::*;

pub(super) fn initialize_result(
    params: &InitializeParams,
    completion_documentation_resolve_supported: &AtomicBool,
) -> InitializeResult {
    completion_documentation_resolve_supported.store(
        client_supports_completion_documentation_resolve(params),
        Ordering::Relaxed,
    );
    InitializeResult {
        server_info: Some(ServerInfo {
            name: "sql-lsp".to_string(),
            version: Some("0.1.0".to_string()),
        }),
        capabilities: ServerCapabilities {
            experimental: Some(
                serde_json::json!({"oxideAnalysisConfiguration":{"version":1},"oxideDiagnosticSnapshots":{"version":1,"syntaxWithoutMetadata":true}}),
            ),
            text_document_sync: Some(TextDocumentSyncCapability::Kind(
                TextDocumentSyncKind::INCREMENTAL,
            )),
            completion_provider: Some(CompletionOptions {
                resolve_provider: Some(true),
                trigger_characters: Some(vec![
                    ".".to_string(),
                    " ".to_string(),
                    "(".to_string(),
                    "@".to_string(),
                ]),
                ..Default::default()
            }),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            signature_help_provider: Some(SignatureHelpOptions {
                trigger_characters: Some(vec![
                    "(".to_string(),
                    ",".to_string(),
                    " ".to_string(),
                    "=".to_string(),
                ]),
                retrigger_characters: Some(vec![",".to_string(), "=".to_string()]),
                ..Default::default()
            }),
            definition_provider: Some(OneOf::Left(true)),
            references_provider: Some(OneOf::Left(true)),
            document_symbol_provider: Some(OneOf::Left(true)),
            workspace_symbol_provider: Some(OneOf::Left(true)),
            code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
                code_action_kinds: Some(vec![
                    CodeActionKind::QUICKFIX,
                    CodeActionKind::REFACTOR_REWRITE,
                    CodeActionKind::SOURCE,
                ]),
                resolve_provider: Some(false),
                ..Default::default()
            })),
            document_formatting_provider: Some(OneOf::Left(true)),
            document_range_formatting_provider: Some(OneOf::Left(true)),
            rename_provider: Some(OneOf::Right(RenameOptions {
                prepare_provider: Some(true),
                work_done_progress_options: WorkDoneProgressOptions::default(),
            })),
            folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
            selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
            semantic_tokens_provider: Some(
                SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
                    legend: semantic_tokens_legend(),
                    range: Some(false),
                    full: Some(SemanticTokensFullOptions::Bool(true)),
                    ..Default::default()
                }),
            ),
            inlay_hint_provider: Some(OneOf::Right(InlayHintServerCapabilities::Options(
                InlayHintOptions {
                    resolve_provider: Some(false),
                    ..Default::default()
                },
            ))),
            diagnostic_provider: Some(DiagnosticServerCapabilities::Options(DiagnosticOptions {
                identifier: Some("sql-lsp".to_string()),
                inter_file_dependencies: true,
                workspace_diagnostics: false,
                ..Default::default()
            })),
            ..Default::default()
        },
    }
}
