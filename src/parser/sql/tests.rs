use super::core::minimal_input_edit;
use super::types::{CompletionContext, RelationAlias, SqlParser};
use tower_lsp::lsp_types::{DiagnosticSeverity, Position, Range};
mod support;
use support::{analyzed_context_at_end, position_at_end};
mod completion;
mod fallback_dml;
mod quoted_names;
mod relations;
mod scopes;
mod search;
mod syntax;
mod syntax_cache;
mod unicode;

mod tsql;

mod joined_tables;
mod lateral_views;
mod qualify;
mod quantified;
mod query_grammar;
mod routine_inputs;
