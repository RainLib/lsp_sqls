//! Runtime for the SQL language server.
use super::builtin_completion::{
    add_builtin_table_function_completions, add_expression_routine_completions,
    add_oracle_table_function_completions,
};
use super::completion_case::apply_completion_preferences;
use super::completion_context::{
    apply_completed_sql_context_completion_edits, apply_high_frequency_keyword_ranking,
    completion_statement_prefix, deduplicate_simple_completion_items, position_to_byte_offset,
    CompletionCandidates,
};
use super::completion_identifiers::{
    add_referenced_alias_completions, apply_qualified_identifier_completion_edits,
};
use super::completion_resolve::CompletionResolveCache;
use super::diagnostics::document_diagnostics_with_metadata;
use super::dialect_resolution::infer_dialect_from_uri_and_language;
use super::documents::{CachedSqlAnalysis, DocumentManager};
use super::inline_types::{
    InlineCompletionCandidate, InlineCompletionContextResponse, InlineCompletionDiagnostic,
    InlineCompletionValidationParams,
};
use super::insert_completion::{add_insert_all_columns_completion, add_insert_statement_snippets};
use super::limits::{PROJECT_SQL_INDEX_MAX_DOCUMENTS, PROJECT_SQL_INDEX_MAX_RESULTS};
use super::local_relations::augment_schema_with_local_relations_and_routines;
use super::locations::range_contains_position;
use super::preferences::{
    dialect_uses_sql_preferences, CompletionPreferences, FormattingPreferences,
};
use super::project_index::{
    documents_share_project_sql_scope, project_sql_symbol_occurrences, project_sql_symbols_match,
};
use super::project_types::{
    CachedProjectSqlIndex, ProjectSqlSymbolOccurrence, ProjectSqlSymbolRole,
};
use super::schema_resolution::{
    find_schema_by_qualifier, schema_for_table_column_at_position, schema_id_for_file,
    schema_qualifier_at_position,
};
use crate::dialect::Dialect;
use crate::dialects::DialectRegistry;
use crate::parser::SqlParser;
use crate::placeholder::SqlPlaceholderDialect;
use crate::position::lsp_position_at_end;
use crate::schema::Schema;
use crate::schema::SchemaId;
use crate::schema::SchemaManager;
use dashmap::DashMap;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::Client;
/// SQL LSP 服务器
pub struct SqlLspServer {
    pub(super) client: Client,
    pub(super) analysis: super::analysis_state::AnalysisState,
    /// 方言注册表
    pub(super) dialect_registry: Arc<DialectRegistry>,
    /// Schema 管理器
    pub(super) schema_manager: Arc<SchemaManager>,
    /// 客户端显式配置的文件到方言映射
    pub(super) configured_file_dialects: Arc<DashMap<String, String>>,
    /// 根据 URI/languageId/默认方言推断出的文件到方言映射
    pub(super) inferred_file_dialects: Arc<DashMap<String, String>>,
    /// 客户端显式配置的文件到 Schema ID 映射
    pub(super) configured_file_schemas: Arc<DashMap<String, SchemaId>>,
    /// 根据文档内容自动推断出的文件到 Schema ID 映射
    pub(super) inferred_file_schemas: Arc<DashMap<String, SchemaId>>,
    /// 无法从 URI/languageId 推断时使用的默认方言
    pub(super) default_dialect: Arc<RwLock<String>>,
    pub(super) completion_preferences: Arc<RwLock<CompletionPreferences>>,
    pub(super) formatting_preferences: Arc<RwLock<FormattingPreferences>>,
    /// 打开文档的 languageId，用于配置刷新后恢复推断方言
    pub(super) document_languages: Arc<DashMap<String, String>>,
    /// 文档管理器
    pub(super) document_manager: DocumentManager,
    pub(super) analysis_cache: Arc<DashMap<String, CachedSqlAnalysis>>,
    pub(super) project_sql_index_cache: Arc<DashMap<String, CachedProjectSqlIndex>>,
    pub(super) completion_resolve_cache: Mutex<CompletionResolveCache>,
    pub(super) next_completion_resolve_id: AtomicU64,
    pub(super) completion_documentation_resolve_supported: AtomicBool,
}

impl SqlLspServer {
    pub fn new(client: Client) -> Self {
        tracing::info!("Creating new SQL LSP server instance");
        Self {
            client,
            analysis: Default::default(),
            dialect_registry: Arc::new(DialectRegistry::new()),
            schema_manager: Arc::new(SchemaManager::new()),
            configured_file_dialects: Arc::new(DashMap::new()),
            inferred_file_dialects: Arc::new(DashMap::new()),
            configured_file_schemas: Arc::new(DashMap::new()),
            inferred_file_schemas: Arc::new(DashMap::new()),
            default_dialect: Arc::new(RwLock::new("postgres".to_string())),
            completion_preferences: Arc::new(RwLock::new(CompletionPreferences::default())),
            formatting_preferences: Arc::new(RwLock::new(FormattingPreferences::default())),
            document_languages: Arc::new(DashMap::new()),
            document_manager: DocumentManager::new(),
            analysis_cache: Arc::new(DashMap::new()),
            project_sql_index_cache: Arc::new(DashMap::new()),
            completion_resolve_cache: Mutex::new(CompletionResolveCache::default()),
            next_completion_resolve_id: AtomicU64::new(1),
            completion_documentation_resolve_supported: AtomicBool::new(false),
        }
    }

    pub(super) fn default_dialect_name(&self) -> String {
        self.default_dialect
            .read()
            .map(|dialect| dialect.clone())
            .unwrap_or_else(|_| "postgres".to_string())
    }

    pub(super) fn inferred_dialect_for_document(&self, uri: &str, language_id: &str) -> String {
        infer_dialect_from_uri_and_language(uri, language_id, &self.default_dialect_name())
    }

    pub(super) fn ensure_dialect_for_document(&self, uri: &str, language_id: &str) {
        if self.configured_file_dialects.contains_key(uri) {
            return;
        }

        self.inferred_file_dialects
            .entry(uri.to_string())
            .or_insert_with(|| self.inferred_dialect_for_document(uri, language_id));
    }

    pub(super) fn ensure_dialects_for_open_documents(&self) {
        for entry in self.document_languages.iter() {
            self.ensure_dialect_for_document(entry.key(), entry.value());
        }
    }

    /// 获取文件的方言
    pub(super) fn get_dialect_for_file(&self, uri: &str) -> Option<Arc<dyn Dialect>> {
        if let Some(name) = self.configured_dialect_identity(uri) {
            return self.dialect_registry.get_by_name(&name);
        }

        self.inferred_file_dialects
            .get(uri)
            .and_then(|dialect_name| self.dialect_registry.get_by_name(dialect_name.value()))
    }

    pub(super) fn dialect_identity_for_file(&self, uri: &str) -> Option<String> {
        self.configured_dialect_identity(uri).or_else(|| {
            self.inferred_file_dialects
                .get(uri)
                .map(|dialect| dialect.value().clone())
        })
    }

    fn configured_dialect_identity(&self, uri: &str) -> Option<String> {
        self.configured_file_dialects
            .get(uri)
            .map(|value| value.clone())
            .or_else(|| {
                let source = super::auxiliary_documents::configured_source_uri(uri)?;
                self.configured_file_dialects
                    .get(&source)
                    .map(|value| value.clone())
            })
    }

    /// 获取文件的 Schema
    /// 如果文件没有显式关联的 Schema，则根据 SQL 内容自动推断最佳匹配的 Schema
    pub(super) fn get_schema_for_file(&self, uri: &str) -> Option<Schema> {
        let text = self.document_manager.get(uri);
        schema_id_for_file(
            uri,
            text.as_deref(),
            &self.configured_file_schemas,
            &self.inferred_file_schemas,
            &self.schema_manager,
        )
        .and_then(|schema_id| self.schema_manager.get(schema_id))
    }

    pub(super) fn get_schema_for_position(
        &self,
        uri: &str,
        text: &str,
        position: Position,
    ) -> Option<Schema> {
        let schema = schema_for_table_column_at_position(&self.schema_manager, text, position)
            .or_else(|| {
                schema_qualifier_at_position(text, position).and_then(|qualifier| {
                    find_schema_by_qualifier(&self.schema_manager, &qualifier)
                })
            })
            .or_else(|| self.get_schema_for_file(uri));
        let has_metadata = schema.is_some();
        let schema = schema.unwrap_or_else(|| Schema {
            id: SchemaId(uuid::Uuid::nil()),
            server: None,
            namespace: None,
            catalog: None,
            database: String::new(),
            server_version: None,
            tables: Vec::new(),
            functions: Vec::new(),
            source_uri: None,
        });
        let dialect = self.dialect_identity_for_file(uri);
        let schema = augment_schema_with_local_relations_and_routines(
            schema,
            text,
            position,
            uri,
            dialect.as_deref(),
            Some(&self.schema_manager),
        );
        // Local projections remain useful while physical metadata is loading.
        // This ephemeral shape is never registered as a synchronized catalog.
        (has_metadata || !schema.tables.is_empty()).then_some(schema)
    }

    /// 将 LSP Position 转换为字符串字节偏移
    pub(super) fn position_to_offset(
        &self,
        text: &str,
        position: tower_lsp::lsp_types::Position,
    ) -> usize {
        position_to_byte_offset(text, position)
    }

    pub(super) fn analysis_tree_for_document(
        &self,
        uri: &str,
        text: &str,
        dialect: &str,
    ) -> Option<tree_sitter::Tree> {
        if !matches!(
            dialect,
            "postgres" | "mysql" | "clickhouse" | "hive" | "sqlserver"
        ) {
            self.analysis_cache.remove(uri);
            return None;
        }
        if let Some(cached) = self.analysis_cache.get(uri) {
            if cached.source == text {
                return Some(cached.tree.clone());
            }
        }
        let placeholder_dialect = SqlPlaceholderDialect::for_name(Some(dialect));
        let mut parser = SqlParser::new_with_placeholder_dialect(placeholder_dialect);
        let result = if let Some(cached) = self.analysis_cache.get(uri) {
            parser.parse_incremental(text, &cached.source, &cached.tree)
        } else {
            parser.parse(text)
        };
        let tree = result.tree?;
        self.analysis_cache.insert(
            uri.to_string(),
            CachedSqlAnalysis {
                source: text.to_string(),
                tree: tree.clone(),
            },
        );
        Some(tree)
    }

    pub(super) fn project_sql_index_for_document(
        &self,
        uri: &str,
        text: &str,
    ) -> Vec<ProjectSqlSymbolOccurrence> {
        let revision = self.document_manager.revision(uri).unwrap_or_default();
        if let Some(cached) = self.project_sql_index_cache.get(uri) {
            if cached.revision == revision {
                return cached.occurrences.clone();
            }
        }
        let occurrences = project_sql_symbol_occurrences(text);
        if self.document_manager.revision(uri) == Some(revision)
            && self
                .document_manager
                .get(uri)
                .as_deref()
                .is_some_and(|current| current == text)
        {
            self.project_sql_index_cache.insert(
                uri.to_string(),
                CachedProjectSqlIndex {
                    revision,
                    occurrences: occurrences.clone(),
                },
            );
        }
        occurrences
    }

    pub(super) fn project_sql_documents(&self) -> Vec<(String, String)> {
        let mut documents = self.document_manager.entries();
        documents.sort_by(|left, right| left.0.cmp(&right.0));
        documents.truncate(PROJECT_SQL_INDEX_MAX_DOCUMENTS);
        documents
    }

    pub(super) fn project_sql_symbol_at_position(
        &self,
        uri: &str,
        text: &str,
        position: Position,
    ) -> Option<ProjectSqlSymbolOccurrence> {
        self.project_sql_index_for_document(uri, text)
            .into_iter()
            .find(|occurrence| range_contains_position(occurrence.range, position))
    }

    pub(super) fn matching_project_sql_locations(
        &self,
        origin_uri: &str,
        origin_dialect: &str,
        origin_schema_id: Option<SchemaId>,
        target: &ProjectSqlSymbolOccurrence,
        include_declarations: bool,
    ) -> Vec<Location> {
        let mut locations = Vec::new();
        for (candidate_uri, candidate_text) in self.project_sql_documents() {
            let Some(candidate_dialect) = self.get_dialect_for_file(&candidate_uri) else {
                continue;
            };
            if candidate_dialect.name() != origin_dialect
                || !documents_share_project_sql_scope(
                    origin_uri,
                    &candidate_uri,
                    origin_schema_id,
                    self.get_schema_for_file(&candidate_uri)
                        .as_ref()
                        .map(|schema| schema.id),
                )
            {
                continue;
            }
            let Ok(candidate_url) = Url::parse(&candidate_uri) else {
                continue;
            };
            for occurrence in self.project_sql_index_for_document(&candidate_uri, &candidate_text) {
                if (!include_declarations && occurrence.role == ProjectSqlSymbolRole::Definition)
                    || !project_sql_symbols_match(target, &occurrence)
                {
                    continue;
                }
                locations.push(Location {
                    uri: candidate_url.clone(),
                    range: occurrence.range,
                });
                if locations.len() >= PROJECT_SQL_INDEX_MAX_RESULTS {
                    return locations;
                }
            }
        }
        locations
    }

    pub(super) fn project_sql_definition(
        &self,
        origin_uri: &str,
        origin_dialect: &str,
        origin_schema_id: Option<SchemaId>,
        target: &ProjectSqlSymbolOccurrence,
    ) -> Option<Location> {
        for (candidate_uri, candidate_text) in self.project_sql_documents() {
            let Some(candidate_dialect) = self.get_dialect_for_file(&candidate_uri) else {
                continue;
            };
            if candidate_dialect.name() != origin_dialect
                || !documents_share_project_sql_scope(
                    origin_uri,
                    &candidate_uri,
                    origin_schema_id,
                    self.get_schema_for_file(&candidate_uri)
                        .as_ref()
                        .map(|schema| schema.id),
                )
            {
                continue;
            }
            let Ok(candidate_url) = Url::parse(&candidate_uri) else {
                continue;
            };
            if let Some(definition) = self
                .project_sql_index_for_document(&candidate_uri, &candidate_text)
                .into_iter()
                .find(|occurrence| {
                    occurrence.role == ProjectSqlSymbolRole::Definition
                        && project_sql_symbols_match(target, occurrence)
                })
            {
                return Some(Location {
                    uri: candidate_url,
                    range: definition.range,
                });
            }
        }
        None
    }

    pub(super) fn project_sql_symbol_is_renameable(
        &self,
        uri: &str,
        dialect: &str,
        schema: Option<&Schema>,
        target: &ProjectSqlSymbolOccurrence,
    ) -> bool {
        if target.kind.is_routine() {
            let metadata_matches = schema.map_or(0, |schema| {
                schema
                    .functions
                    .iter()
                    .filter(|function| function.name.eq_ignore_ascii_case(&target.name))
                    .count()
            });
            let project_definitions = self.project_sql_definition_count(
                uri,
                dialect,
                schema.map(|schema| schema.id),
                target,
            );
            return metadata_matches <= 1
                && project_definitions <= 1
                && (metadata_matches == 1 || project_definitions == 1);
        }
        let metadata_match = schema.is_some_and(|schema| {
            schema.tables.iter().any(|table| {
                SqlParser::table_name_matches_with_catalog(
                    &target.normalized_name,
                    schema.catalog.as_deref(),
                    &schema.database,
                    &table.name,
                )
            })
        });
        metadata_match
            || self
                .project_sql_definition(uri, dialect, schema.map(|schema| schema.id), target)
                .is_some()
    }

    pub(super) fn project_sql_definition_count(
        &self,
        origin_uri: &str,
        origin_dialect: &str,
        origin_schema_id: Option<SchemaId>,
        target: &ProjectSqlSymbolOccurrence,
    ) -> usize {
        let mut count = 0;
        for (candidate_uri, candidate_text) in self.project_sql_documents() {
            let Some(candidate_dialect) = self.get_dialect_for_file(&candidate_uri) else {
                continue;
            };
            if candidate_dialect.name() != origin_dialect
                || !documents_share_project_sql_scope(
                    origin_uri,
                    &candidate_uri,
                    origin_schema_id,
                    self.get_schema_for_file(&candidate_uri)
                        .as_ref()
                        .map(|schema| schema.id),
                )
            {
                continue;
            }
            count += self
                .project_sql_index_for_document(&candidate_uri, &candidate_text)
                .into_iter()
                .filter(|occurrence| {
                    occurrence.role == ProjectSqlSymbolRole::Definition
                        && project_sql_symbols_match(target, occurrence)
                })
                .take(2 - count)
                .count();
            if count >= 2 {
                return count;
            }
        }
        count
    }

    pub(super) async fn completion_candidates_at(
        &self,
        uri: &str,
        text: &str,
        position: Position,
        context: Option<&CompletionContext>,
    ) -> Option<CompletionCandidates> {
        let dialect = self.get_dialect_for_file(uri)?;
        let dialect_identity = self
            .dialect_identity_for_file(uri)
            .unwrap_or_else(|| dialect.name().to_string());
        if dialect_uses_sql_preferences(dialect.name())
            && super::completion_regions::in_literal_or_comment(text, position, &dialect_identity)
        {
            return Some(Vec::new().into());
        }
        if let Some(mut candidates) = super::is_predicate_completion::completion_candidates(
            text,
            position,
            &dialect_identity,
            || self.get_schema_for_position(uri, text, position),
        ) {
            let preferences = self
                .completion_preferences
                .read()
                .map(|value| value.clone())
                .unwrap_or_default();
            apply_completion_preferences(text, position, &mut candidates.items, &preferences);
            return Some(candidates);
        }
        if let Some(mut items) = super::execute_completion::completions(
            text,
            position,
            &dialect_identity,
            || self.get_schema_for_file(uri),
            &self.schema_manager,
        ) {
            let preferences = self
                .completion_preferences
                .read()
                .map(|p| p.clone())
                .unwrap_or_default();
            apply_completion_preferences(text, position, &mut items.items, &preferences);
            return Some(items);
        }
        if let Some(items) = super::call_completion::completions(
            text,
            position,
            &dialect_identity,
            || self.get_schema_for_file(uri),
            &self.schema_manager,
        ) {
            return Some(items);
        }
        if let Some(items) =
            super::tsql_variable_completion::completions(text, position, &dialect_identity)
        {
            return Some(items);
        }
        let schema = self.get_schema_for_position(uri, text, position);
        let (completion_text, completion_position) =
            completion_statement_prefix(text, position).unwrap_or((text, position));
        let completion = dialect
            .completion_with_scope(
                completion_text,
                completion_position,
                schema.as_ref(),
                context,
            )
            .await;
        let mut items = completion.items;
        if let Some(server) = schema.as_ref().and_then(|schema| schema.server.as_ref()) {
            for item in &mut items {
                if item.kind == Some(CompletionItemKind::FIELD) {
                    if let Some(source) = item.data.as_mut().and_then(|data| data.get_mut("oxide"))
                    {
                        if source.get("column").is_some() {
                            source["server"] = serde_json::json!(server);
                        }
                    }
                }
            }
        }
        let qualify_incomplete = super::local_relations::add_qualify_completions(
            text,
            position,
            &dialect_identity,
            schema.as_ref(),
            uri,
            Some(&self.schema_manager),
            &mut items,
        );
        add_expression_routine_completions(
            text,
            position,
            &dialect_identity,
            schema.as_ref(),
            &mut items,
            Some((completion_text, completion_position)),
        );
        add_builtin_table_function_completions(
            completion_text,
            completion_position,
            &dialect_identity,
            &mut items,
        );
        add_oracle_table_function_completions(
            completion_text,
            completion_position,
            &dialect_identity,
            schema.as_ref(),
            &mut items,
        );
        let expectation = super::comparison_value_types::enrich(
            text,
            position,
            &dialect_identity,
            schema.as_ref(),
            &mut items,
        );
        let (value_type, value_role) =
            expectation.map_or((None, None), |value| (Some(value.data_type), value.role));
        let continuation = if value_type.is_none() {
            super::logical_continuations::enrich(
                text,
                position,
                &dialect_identity,
                schema.as_ref(),
                &mut items,
            )
        } else {
            None
        };
        let logical_continuation = continuation.map(|knowledge| knowledge.logical);
        let pattern_escape = continuation.and_then(|knowledge| knowledge.pattern_escape);
        let preferences = self
            .completion_preferences
            .read()
            .map(|preferences| preferences.clone())
            .unwrap_or_default();
        let relational_completion = dialect_uses_sql_preferences(dialect.name());
        if relational_completion {
            add_insert_statement_snippets(
                text,
                position,
                schema.as_ref(),
                &mut items,
                &preferences,
                dialect.name(),
            );
            add_insert_all_columns_completion(
                text,
                position,
                schema.as_ref(),
                &mut items,
                &preferences,
                dialect.name(),
            );
        }
        add_referenced_alias_completions(
            text,
            position,
            &mut items,
            completion.relation_aliases.as_deref(),
        );
        apply_completed_sql_context_completion_edits(text, position, &mut items);
        if relational_completion {
            let quote_dialect =
                if matches!(dialect_identity.as_str(), "sqlserver" | "mssql" | "tsql") {
                    "sqlserver"
                } else {
                    dialect.name()
                };
            super::completion_quotes::apply_column_identifier_edits(
                text,
                position,
                quote_dialect,
                &mut items,
            );
        }
        apply_qualified_identifier_completion_edits(text, position, &mut items);
        if relational_completion {
            apply_completion_preferences(text, position, &mut items, &preferences);
        }
        deduplicate_simple_completion_items(&mut items);
        apply_high_frequency_keyword_ranking(&mut items);
        let named = super::named_completion::completions(
            text,
            position,
            &dialect_identity,
            || self.get_schema_for_file(uri),
            &self.schema_manager,
        );
        items.extend(named.items);
        let prefix = crate::dialects::common::cursor_prefix_for_dialect(
            crate::placeholder::SqlPlaceholderDialect::for_name(Some(&dialect_identity)),
            text,
            crate::parser::SqlParser::lsp_position_to_byte_position(text, position),
        );
        let search_incomplete = if !relational_completion
            || super::completion_context::COMPLETED_SQL_CONTEXT_KEYWORDS.contains(&prefix.as_str())
        {
            false
        } else {
            crate::identifier_search::rank_items(&mut items, &prefix)
        };
        Some(CompletionCandidates {
            value_type,
            value_role,
            logical_continuation,
            pattern_escape,
            constraints: None,
            items,
            is_incomplete: named.is_incomplete || qualify_incomplete || search_incomplete,
        })
    }

    pub async fn inline_completion_context(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<InlineCompletionContextResponse> {
        let _analysis = self.analysis.configuration.read().await;
        let uri = params.text_document.uri.to_string();
        let position = params.position;
        let text = self.document_manager.get(&uri).unwrap_or_default();
        let Some(dialect) = self.get_dialect_for_file(&uri) else {
            return Ok(empty_inline_completion_context());
        };
        let schema = self.get_schema_for_position(&uri, &text, position);
        let mut raw_candidates = self
            .completion_candidates_at(&uri, &text, position, None)
            .await
            .unwrap_or_default()
            .items;
        raw_candidates.sort_by(|left, right| {
            left.sort_text
                .as_deref()
                .unwrap_or(&left.label)
                .cmp(right.sort_text.as_deref().unwrap_or(&right.label))
                .then_with(|| left.label.cmp(&right.label))
        });
        let candidates = raw_candidates
            .into_iter()
            .take(32)
            .map(|item| InlineCompletionCandidate {
                label: item.label.clone(),
                insert_text: completion_item_effective_insert_text(&item),
                kind: item.kind.and_then(serialized_lsp_number),
            })
            .collect();
        let raw_diagnostics = document_diagnostics_with_metadata(
            &*dialect,
            &text,
            schema.as_ref(),
            Some(&self.schema_manager),
        )
        .await;
        let error_count = raw_diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Some(DiagnosticSeverity::ERROR))
            .count();
        let diagnostics = raw_diagnostics
            .into_iter()
            .take(16)
            .map(|diagnostic| InlineCompletionDiagnostic {
                severity: diagnostic.severity.and_then(serialized_lsp_number),
                message: diagnostic.message,
            })
            .collect();

        let parser = SqlParser::new();
        let byte_position = SqlParser::lsp_position_to_byte_position(&text, position);
        let (clause, statement_range, referenced_objects, aliases, ctes) =
            if let Some(tree) = self.analysis_tree_for_document(&uri, &text, dialect.name()) {
                let node = parser.get_node_at_position(&tree, byte_position);
                let context =
                    node.map(|node| parser.analyze_completion_context(node, &text, byte_position));
                (
                    context
                        .as_ref()
                        .map(|context| format!("{context:?}"))
                        .unwrap_or_else(|| "Default".to_string()),
                    node.and_then(|node| statement_range_for_node(&text, node)),
                    parser.extract_referenced_tables_at_position(&tree, &text, byte_position),
                    parser.extract_aliases_at_position(&tree, &text, byte_position),
                    parser.extract_common_table_expressions(&text),
                )
            } else {
                (
                    "Default".to_string(),
                    None,
                    Vec::new(),
                    HashMap::new(),
                    Vec::new(),
                )
            };
        let expected_kinds = expected_kinds_for_clause(&clause);
        let statement_range = statement_range.or_else(|| {
            (!text.is_empty()).then_some(Range {
                start: Position::new(0, 0),
                end: lsp_position_at_end(&text),
            })
        });

        Ok(InlineCompletionContextResponse {
            dialect: dialect.name().to_string(),
            clause,
            statement_range,
            ctes,
            referenced_objects,
            aliases,
            expected_kinds,
            candidates,
            diagnostics,
            error_count,
        })
    }

    pub async fn validate_inline_completion(
        &self,
        params: InlineCompletionValidationParams,
    ) -> Result<Vec<Diagnostic>> {
        let _analysis = self.analysis.configuration.read().await;
        let uri = params.text_document.uri.to_string();
        let Some(dialect) = self.get_dialect_for_file(&uri) else {
            return Ok(Vec::new());
        };
        let schema = self.get_schema_for_file(&uri);
        Ok(document_diagnostics_with_metadata(
            &*dialect,
            &params.text,
            schema.as_ref(),
            Some(&self.schema_manager),
        )
        .await)
    }
}

pub(super) fn empty_inline_completion_context() -> InlineCompletionContextResponse {
    InlineCompletionContextResponse {
        dialect: "unknown".to_string(),
        clause: "Default".to_string(),
        statement_range: None,
        ctes: Vec::new(),
        referenced_objects: Vec::new(),
        aliases: HashMap::new(),
        expected_kinds: Vec::new(),
        candidates: Vec::new(),
        diagnostics: Vec::new(),
        error_count: 0,
    }
}

pub(super) fn serialized_lsp_number(value: impl Serialize) -> Option<u32> {
    serde_json::to_value(value).ok()?.as_u64()?.try_into().ok()
}

pub(super) fn completion_item_effective_insert_text(item: &CompletionItem) -> Option<String> {
    item.text_edit
        .as_ref()
        .map(|edit| match edit {
            CompletionTextEdit::Edit(edit) => edit.new_text.clone(),
            CompletionTextEdit::InsertAndReplace(edit) => edit.new_text.clone(),
        })
        .or_else(|| item.insert_text.clone())
        .or_else(|| Some(item.label.clone()))
}

pub(super) fn statement_range_for_node(
    source: &str,
    mut node: tree_sitter::Node<'_>,
) -> Option<Range> {
    loop {
        let kind = node.kind();
        if kind == "statement" || kind.ends_with("_statement") {
            let start = node.start_position();
            let end = node.end_position();
            return Some(Range {
                start: crate::position::byte_position_to_lsp_position(
                    source,
                    Position::new(start.row as u32, start.column as u32),
                ),
                end: crate::position::byte_position_to_lsp_position(
                    source,
                    Position::new(end.row as u32, end.column as u32),
                ),
            });
        }
        node = node.parent()?;
    }
}

pub(super) fn expected_kinds_for_clause(clause: &str) -> Vec<String> {
    let values: &[&str] = match clause {
        "FromClause" | "JoinClause" => &["relation", "schema"],
        "TableColumn" | "SelectClause" | "WhereClause" | "HavingClause" | "OrderByClause"
        | "GroupByClause" | "UsingClause" => &["column", "function", "keyword"],
        "DataTypeClause" => &["data-type"],
        "ExpressionValueClause" | "InsertValueClause" | "CaseResultClause" => {
            &["value", "function", "placeholder"]
        }
        _ => &["keyword", "relation", "column"],
    };
    values.iter().map(|value| (*value).to_string()).collect()
}
