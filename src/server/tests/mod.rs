//! Shared SQL semantic fixtures.

use super::builtin_completion::{
    add_builtin_function_completions, add_clickhouse_table_function_completions,
    add_oracle_table_function_completions, add_window_function_completions,
    oracle_table_routine_context,
};

use super::code_actions::{
    code_action_kind_explicitly_requested, expand_select_star_action, qualify_identifier_actions,
};

use super::completion_case::apply_completion_preferences;

use super::completion_context::{
    apply_completed_sql_context_completion_edits, apply_high_frequency_keyword_ranking,
    completed_sql_context_keyword_at_position, completion_statement_prefix,
    deduplicate_simple_completion_items, position_to_byte_offset,
};

use super::completion_identifiers::{
    add_referenced_alias_completions, apply_qualified_identifier_completion_edits,
    table_alias_initials,
};

use super::completion_resolve::{
    client_supports_completion_documentation_resolve, defer_completion_documentation,
    resolve_completion_documentation, CompletionResolveCache,
};

use super::diagnostics::sql_inspection_diagnostics;

use super::dialect_resolution::infer_dialect_from_uri_and_language;

use super::document_structure::range_for_offsets;

use super::insert_completion::{add_insert_all_columns_completion, add_insert_statement_snippets};

use super::insert_hints::insert_value_hints;

use super::join_actions::add_join_condition_actions;

use super::limits::{
    COMPLETION_RESOLVE_CACHE_MAX_ENTRIES, COMPLETION_RESOLVE_DOCUMENTATION_MAX_BYTES,
    PROJECT_SQL_INDEX_MAX_BYTES,
};

use super::local_relations::{augment_schema_with_local_relations, LOCAL_RELATION_SCAN_MAX_BYTES};

use super::locations::{
    rewrite_current_document_location_uri, rewrite_current_document_location_uris,
};

use super::preferences::{
    CompletionPreferences, FormattingPreferences, FromClauseLayout, KeywordCase,
    LogicalOperatorNewline, TableAliasStyle,
};

use super::project_index::{project_sql_symbol_occurrences, project_sql_symbols_match};

use super::project_types::{
    ProjectSqlSymbolKind, ProjectSqlSymbolOccurrence, ProjectSqlSymbolRole,
};

use super::schema_resolution::{
    calculate_schema_match_score, find_schema_by_qualifier, find_schema_by_table_reference,
    infer_schema_id_from_tables, schema_for_table_column_at_position, schema_id_for_file,
    schema_qualifier_at_position, PRODUCT_SQL_FILE_SUFFIX_DIALECTS,
};

use super::signature_help::{
    code_action_kind_available, live_overload_accepts_call, routine_call_at_position,
    RoutineCallContext,
};

use crate::dialects::DialectRegistry;

use crate::position::lsp_position_at_end;

use crate::schema::Column;

use crate::schema::Constraint;

use crate::schema::Function;

use crate::schema::FunctionParameter;

use crate::schema::Schema;

use crate::schema::SchemaId;

use crate::schema::SchemaManager;

use crate::schema::Table;

use dashmap::DashMap;

use std::sync::atomic::AtomicU64;

use std::sync::Mutex;

use tower_lsp::lsp_types::CodeAction;

use tower_lsp::lsp_types::CodeActionContext;

use tower_lsp::lsp_types::CodeActionKind;

use tower_lsp::lsp_types::CompletionItem;

use tower_lsp::lsp_types::CompletionItemKind;

use tower_lsp::lsp_types::CompletionTextEdit;

use tower_lsp::lsp_types::Documentation;

use tower_lsp::lsp_types::InitializeParams;

use tower_lsp::lsp_types::InlayHintLabel;

use tower_lsp::lsp_types::InsertTextFormat;

use tower_lsp::lsp_types::Location;

use tower_lsp::lsp_types::Position;

use tower_lsp::lsp_types::Range;

use tower_lsp::lsp_types::TextEdit;

use tower_lsp::lsp_types::Url;

fn test_schema(database: &str, tables: &[&str]) -> Schema {
    Schema {
        id: SchemaId::new(),
        catalog: None,
        database: database.to_string(),
        server_version: None,
        tables: tables
            .iter()
            .map(|name| Table {
                name: (*name).to_string(),
                ..Default::default()
            })
            .collect(),
        functions: Vec::new(),
        source_uri: None,
    }
}

fn test_location(uri: &str) -> Location {
    Location {
        uri: Url::parse(uri).expect("valid test URI"),
        range: Range::default(),
    }
}

fn diagnostic_codes(diagnostics: &[tower_lsp::lsp_types::Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .filter_map(|diagnostic| match diagnostic.code.as_ref() {
            Some(tower_lsp::lsp_types::NumberOrString::String(code)) => Some(code.clone()),
            _ => None,
        })
        .collect()
}

fn semantic_test_schema() -> Schema {
    Schema {
        id: SchemaId::new(),
        catalog: None,
        database: "app".to_string(),
        server_version: None,
        tables: vec![Table {
            name: "users".to_string(),
            columns: vec![
                Column {
                    name: "id".to_string(),
                    data_type: "bigint".to_string(),
                    ..Default::default()
                },
                Column {
                    name: "name".to_string(),
                    data_type: "text".to_string(),
                    ..Default::default()
                },
                Column {
                    name: "email".to_string(),
                    data_type: "text".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }],
        functions: Vec::new(),
        source_uri: None,
    }
}

fn completion_schema_with_columns() -> Schema {
    Schema {
        id: SchemaId::new(),
        catalog: None,
        database: "app".to_string(),
        server_version: None,
        tables: vec![
            Table {
                name: "orders".to_string(),
                columns: ["id", "customer_id", "total"]
                    .into_iter()
                    .map(|name| Column {
                        name: name.to_string(),
                        data_type: "bigint".to_string(),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            },
            Table {
                name: "customers".to_string(),
                columns: ["id", "name"]
                    .into_iter()
                    .map(|name| Column {
                        name: name.to_string(),
                        data_type: "text".to_string(),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            },
        ],
        functions: Vec::new(),
        source_uri: None,
    }
}

fn inlay_labels(sql: &str, schema: &Schema) -> Vec<String> {
    insert_value_hints(
        sql,
        Range {
            start: Position::new(0, 0),
            end: lsp_position_at_end(sql),
        },
        schema,
    )
    .into_iter()
    .filter_map(|hint| match hint.label {
        InlayHintLabel::String(label) => Some(label),
        InlayHintLabel::LabelParts(_) => None,
    })
    .collect()
}

fn code_action_edits(action: &CodeAction, uri: &Url) -> Vec<TextEdit> {
    action
        .edit
        .as_ref()
        .and_then(|edit| edit.changes.as_ref())
        .and_then(|changes| changes.get(uri))
        .cloned()
        .expect("code action replacement")
}

fn code_action_replacement(action: &CodeAction, uri: &Url) -> String {
    code_action_edits(action, uri)[0].new_text.clone()
}

mod contracts;

mod completion_resolve;

mod builtin_completion;

mod signature_help;

mod local_relations;

mod locations;

mod dialect_resolution;

mod diagnostics;

mod schema_resolution;

mod completion_context;

mod completion_case;

mod completion_identifiers;

mod insert_hints;

mod insert_completion;

mod join_actions;

mod code_actions;

mod project_index;
