//! Protocol for the SQL language server.
use super::code_actions::{
    add_mutation_safety_guard_action, code_action_kind_explicitly_requested,
    expand_select_star_action, qualify_identifier_actions,
};
use super::completion_context::position_to_byte_offset;
use super::completion_resolve::{defer_completion_documentation, resolve_completion_documentation};
use super::diagnostics::document_diagnostics;
use super::document_structure::{document_symbols, folding_ranges, selection_range_for_position};
use super::insert_hints::insert_value_hints;
use super::join_actions::add_join_condition_actions;
use super::limits::{
    INLAY_HINT_MAX_ITEMS, PROJECT_SQL_INDEX_MAX_RESULTS, SIGNATURE_HELP_MAX_OVERLOADS,
};
use super::locations::{
    cte_identifier_range_at_position, deduplicate_locations, rewrite_current_document_location_uri,
    rewrite_current_document_location_uris, supports_semantic_rename, valid_renamed_identifier,
    workspace_symbol_matches, workspace_symbol_rank,
};
use super::preferences::{
    dialect_uses_sql_preferences, CompletionPreferences, FormattingPreferences, FromClauseLayout,
};
use super::project_index::project_sql_symbol_container;
use super::project_types::ProjectSqlSymbolRole;
use super::routine_hints::routine_parameter_hints;
use super::runtime::SqlLspServer;
use super::semantic_tokens::schema_semantic_tokens;
use super::signature_help::{
    builtin_overload_accepts_call, builtin_signature_information, code_action_kind_available,
    live_overload_accepts_call, live_signature_information, routine_call_at_position,
    routine_names_match, single_document_edit,
};
use crate::builtin_signatures::builtin_signatures_for;
use crate::dialects::common::apply_formatter_function_case;
use crate::dialects::common::apply_formatter_layout;
use crate::dialects::common::format_sql_pretty_with_keyword_case;
use crate::position::lsp_position_at_end;
use crate::schema::Schema;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::LanguageServer;

mod capabilities;
#[tower_lsp::async_trait]
impl LanguageServer for SqlLspServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        Ok(capabilities::initialize_result(
            &params,
            &self.completion_documentation_resolve_supported,
        ))
    }

    async fn initialized(&self, _: InitializedParams) {
        tracing::info!("SQL LSP server initialized and ready");
        self.client
            .log_message(MessageType::INFO, "SQL LSP server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let uri = params.text_document.uri.to_string();
        let text = self.document_manager.get(&uri).unwrap_or_default();
        let diagnostics = if let Some(dialect) = self.get_dialect_for_file(&uri) {
            let schema = self.get_schema_for_file(&uri);
            document_diagnostics(&*dialect, &text, schema.as_ref()).await
        } else {
            Vec::new()
        };
        Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: None,
                    items: diagnostics,
                },
            }),
        ))
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        tracing::debug!("Received configuration change");
        // 解析配置 JSON
        if let Some(settings) = params.settings.as_object() {
            // 处理 schemas 配置
            if let Some(default_dialect_value) = settings.get("defaultDialect") {
                if let Some(default_dialect) = default_dialect_value.as_str() {
                    if self.dialect_registry.get_by_name(default_dialect).is_some() {
                        if let Ok(mut current_default) = self.default_dialect.write() {
                            *current_default = default_dialect.to_string();
                        }
                        self.client
                            .log_message(
                                MessageType::INFO,
                                format!("Updated default dialect to {}", default_dialect),
                            )
                            .await;
                    } else {
                        self.client
                            .log_message(
                                MessageType::WARNING,
                                format!(
                                    "Ignoring unsupported default dialect: {}",
                                    default_dialect
                                ),
                            )
                            .await;
                    }
                }
            }

            if let Some(preferences_value) = settings.get("completionPreferences") {
                match serde_json::from_value::<CompletionPreferences>(preferences_value.clone()) {
                    Ok(preferences) => {
                        if let Ok(mut current_preferences) = self.completion_preferences.write() {
                            *current_preferences = preferences;
                        }
                    }
                    Err(error) => {
                        self.client
                            .log_message(
                                MessageType::WARNING,
                                format!("Ignoring invalid completion preferences: {error}"),
                            )
                            .await;
                    }
                }
            }

            if let Some(preferences_value) = settings.get("formattingPreferences") {
                match serde_json::from_value::<FormattingPreferences>(preferences_value.clone()) {
                    Ok(preferences) => {
                        if let Ok(mut current_preferences) = self.formatting_preferences.write() {
                            *current_preferences = preferences;
                        }
                    }
                    Err(error) => {
                        self.client
                            .log_message(
                                MessageType::WARNING,
                                format!("Ignoring invalid formatting preferences: {error}"),
                            )
                            .await;
                    }
                }
            }

            // 处理 schemas 配置
            if let Some(schemas_value) = settings.get("schemas") {
                if let Ok(schemas) =
                    serde_json::from_value::<Vec<crate::schema::Schema>>(schemas_value.clone())
                {
                    // 清空旧的 schema 并注册新的
                    self.schema_manager.clear();
                    let count = schemas.len();
                    for schema in schemas {
                        self.schema_manager.register(schema);
                    }
                    self.inferred_file_schemas.clear();
                    self.client
                        .log_message(MessageType::INFO, format!("Updated {} schemas", count))
                        .await;
                } else {
                    self.client
                        .log_message(
                            MessageType::WARNING,
                            "Failed to parse schemas configuration",
                        )
                        .await;
                }
            }

            // 处理文件到 schema 的映射配置
            if let Some(file_schemas_value) = settings.get("fileSchemas") {
                self.configured_file_schemas.clear();
                self.inferred_file_schemas.clear();
                if let Some(file_schemas_obj) = file_schemas_value.as_object() {
                    for (uri, schema_id_str) in file_schemas_obj {
                        if let Some(id_str) = schema_id_str.as_str() {
                            if let Ok(schema_id) = id_str.parse::<crate::schema::SchemaId>() {
                                self.configured_file_schemas.insert(uri.clone(), schema_id);
                            }
                        }
                    }
                    self.client
                        .log_message(MessageType::INFO, "Updated file-schema mappings")
                        .await;
                }
            }

            // 处理文件到 dialect 的显式映射配置
            if let Some(file_dialects_value) = settings.get("fileDialects") {
                self.configured_file_dialects.clear();
                if let Some(file_dialects_obj) = file_dialects_value.as_object() {
                    for (uri, dialect_name) in file_dialects_obj {
                        if let Some(name) = dialect_name.as_str() {
                            if self.dialect_registry.get_by_name(name).is_some() {
                                self.configured_file_dialects
                                    .insert(uri.clone(), name.to_string());
                            }
                        }
                    }
                    self.client
                        .log_message(MessageType::INFO, "Updated file-dialect mappings")
                        .await;
                }
            }

            self.inferred_file_dialects.clear();
            self.ensure_dialects_for_open_documents();
            self.publish_diagnostics_for_open_documents().await;
        }
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let text = params.text_document.text.clone();
        let language_id = params.text_document.language_id.clone();

        self.inferred_file_schemas.remove(&uri);

        // 存储文档内容
        self.document_manager
            .update(uri.clone(), text.clone(), params.text_document.version);
        self.document_languages
            .insert(uri.clone(), language_id.clone());

        // 尝试从 URI 和 languageId 推断方言。客户端显式配置的 fileDialects
        // 优先级更高，因此只在当前文件没有映射时写入推断结果。
        self.ensure_dialect_for_document(&uri, &language_id);

        // 发布诊断
        if let Some(dialect) = self.get_dialect_for_file(&uri) {
            let schema = self.get_schema_for_file(&uri);
            let diagnostics = document_diagnostics(&*dialect, &text, schema.as_ref()).await;
            self.client
                .publish_diagnostics(
                    params.text_document.uri,
                    diagnostics,
                    self.document_manager.version(&uri),
                )
                .await;
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        self.inferred_file_schemas.remove(&uri);

        // 处理增量同步
        for change in params.content_changes {
            if let Some(range) = change.range {
                // 增量更新：应用部分文本变更
                if let Some(mut current_text) = self.document_manager.get(&uri) {
                    // 将 LSP Range 转换为字节偏移
                    let start_offset = self.position_to_offset(&current_text, range.start);
                    let end_offset = self.position_to_offset(&current_text, range.end);

                    // 应用变更
                    current_text.replace_range(start_offset..end_offset, &change.text);
                    self.document_manager.update(
                        uri.clone(),
                        current_text.clone(),
                        params.text_document.version,
                    );

                    // 重新解析并发布诊断
                    if let Some(dialect) = self.get_dialect_for_file(&uri) {
                        let schema = self.get_schema_for_file(&uri);
                        let diagnostics =
                            document_diagnostics(&*dialect, &current_text, schema.as_ref()).await;
                        self.client
                            .publish_diagnostics(
                                params.text_document.uri.clone(),
                                diagnostics,
                                self.document_manager.version(&uri),
                            )
                            .await;
                    }
                }
            } else {
                // 完整文档更新
                let text = change.text.clone();
                self.document_manager.update(
                    uri.clone(),
                    text.clone(),
                    params.text_document.version,
                );

                if let Some(dialect) = self.get_dialect_for_file(&uri) {
                    let schema = self.get_schema_for_file(&uri);
                    let diagnostics = document_diagnostics(&*dialect, &text, schema.as_ref()).await;
                    self.client
                        .publish_diagnostics(
                            params.text_document.uri.clone(),
                            diagnostics,
                            self.document_manager.version(&uri),
                        )
                        .await;
                }
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        // 清理文档
        self.document_manager.remove(&uri);
        self.document_languages.remove(&uri);
        self.inferred_file_dialects.remove(&uri);
        self.inferred_file_schemas.remove(&uri);
        self.analysis_cache.remove(&uri);
        self.project_sql_index_cache.remove(&uri);
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri.to_string();
        let position = params.text_document_position.position;

        let text = self.document_manager.get(&uri).unwrap_or_default();

        let Some(mut items) = self
            .completion_candidates_at(&uri, &text, position, params.context.as_ref())
            .await
        else {
            return Ok(None);
        };
        if self
            .completion_documentation_resolve_supported
            .load(Ordering::Relaxed)
        {
            defer_completion_documentation(
                &mut items,
                &self.completion_resolve_cache,
                &self.next_completion_resolve_id,
            );
        }
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn completion_resolve(&self, item: CompletionItem) -> Result<CompletionItem> {
        Ok(resolve_completion_documentation(
            item,
            &self.completion_resolve_cache,
        ))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let position = params.text_document_position_params.position;

        let text = self.document_manager.get(&uri).unwrap_or_default();

        if let Some(dialect) = self.get_dialect_for_file(&uri) {
            let schema = self.get_schema_for_position(&uri, &text, position);
            return Ok(dialect.hover(&text, position, schema.as_ref()).await);
        }

        Ok(None)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let document_uri = params.text_document_position_params.text_document.uri;
        let uri = document_uri.to_string();
        let position = params.text_document_position_params.position;

        let text = self.document_manager.get(&uri).unwrap_or_default();

        if let Some(dialect) = self.get_dialect_for_file(&uri) {
            let schema = self.get_schema_for_position(&uri, &text, position);
            if let Some(mut location) = dialect
                .goto_definition(&text, position, schema.as_ref())
                .await
            {
                rewrite_current_document_location_uri(&mut location, &document_uri);
                return Ok(Some(GotoDefinitionResponse::Scalar(location)));
            }
            if let Some(target) = self.project_sql_symbol_at_position(&uri, &text, position) {
                if let Some(location) = self.project_sql_definition(
                    &uri,
                    dialect.name(),
                    schema.as_ref().map(|schema| schema.id),
                    &target,
                ) {
                    return Ok(Some(GotoDefinitionResponse::Scalar(location)));
                }
            }
        }

        Ok(None)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let document_uri = params.text_document_position.text_document.uri;
        let uri = document_uri.to_string();
        let position = params.text_document_position.position;
        let include_declarations = params.context.include_declaration;

        let text = self.document_manager.get(&uri).unwrap_or_default();

        if let Some(dialect) = self.get_dialect_for_file(&uri) {
            let schema = self.get_schema_for_position(&uri, &text, position);
            let mut locations = dialect.references(&text, position, schema.as_ref()).await;
            rewrite_current_document_location_uris(&mut locations, &document_uri);
            if let Some(target) = self.project_sql_symbol_at_position(&uri, &text, position) {
                locations = self.matching_project_sql_locations(
                    &uri,
                    dialect.name(),
                    schema.as_ref().map(|schema| schema.id),
                    &target,
                    include_declarations,
                );
            }
            deduplicate_locations(&mut locations);
            locations.truncate(PROJECT_SQL_INDEX_MAX_RESULTS);
            return Ok(Some(locations));
        }

        Ok(None)
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let position = params.text_document_position_params.position;
        let text = self.document_manager.get(&uri).unwrap_or_default();
        let Some(call) = routine_call_at_position(&text, position) else {
            return Ok(None);
        };
        let schema = self.get_schema_for_position(&uri, &text, position);
        let mut live_overloads = schema
            .as_ref()
            .map(|schema| {
                schema
                    .functions
                    .iter()
                    .filter(|function| routine_names_match(&function.name, &call.name))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        live_overloads.sort_by_key(|function| !live_overload_accepts_call(function, &call));
        live_overloads.truncate(SIGNATURE_HELP_MAX_OVERLOADS);

        let mut signatures = live_overloads
            .iter()
            .filter(|_| call.active_group == 0)
            .map(|function| live_signature_information(function, &call))
            .collect::<Vec<_>>();
        if signatures.is_empty() {
            let dialect = self
                .dialect_identity_for_file(&uri)
                .or_else(|| {
                    self.get_dialect_for_file(&uri)
                        .map(|dialect| dialect.name().into())
                })
                .unwrap_or_else(|| self.default_dialect_name());
            let mut builtins = builtin_signatures_for(
                &dialect,
                &call.name,
                schema.as_ref().and_then(Schema::server_version_tuple),
            );
            builtins.sort_by_key(|signature| !builtin_overload_accepts_call(signature, &call));
            builtins.truncate(SIGNATURE_HELP_MAX_OVERLOADS);
            signatures = builtins
                .iter()
                .filter_map(|signature| builtin_signature_information(signature, &call, &dialect))
                .collect();
        }
        if signatures.is_empty() {
            return Ok(None);
        }
        let active_parameter = signatures[0]
            .active_parameter
            .unwrap_or(call.active_parameter);
        Ok(Some(SignatureHelp {
            signatures,
            active_signature: Some(0),
            active_parameter: Some(active_parameter),
        }))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri.to_string();
        let text = self.document_manager.get(&uri).unwrap_or_default();
        let Some(dialect) = self.get_dialect_for_file(&uri) else {
            return Ok(None);
        };
        if !supports_semantic_rename(dialect.name()) {
            return Ok(None);
        }
        if let Some(target) = self.project_sql_symbol_at_position(&uri, &text, params.position) {
            let schema = self.get_schema_for_position(&uri, &text, params.position);
            return Ok(self
                .project_sql_symbol_is_renameable(&uri, dialect.name(), schema.as_ref(), &target)
                .then_some(PrepareRenameResponse::Range(target.range)));
        }
        let Some(range) = cte_identifier_range_at_position(&text, params.position, dialect.name())
        else {
            return Ok(None);
        };
        let schema = self.get_schema_for_position(&uri, &text, params.position);
        Ok((!dialect
            .references(&text, params.position, schema.as_ref())
            .await
            .is_empty())
        .then_some(PrepareRenameResponse::Range(range)))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri.to_string();
        let position = params.text_document_position.position;
        let text = self.document_manager.get(&uri).unwrap_or_default();
        let Some(dialect) = self.get_dialect_for_file(&uri) else {
            return Ok(None);
        };
        if !supports_semantic_rename(dialect.name()) || !valid_renamed_identifier(&params.new_name)
        {
            return Ok(None);
        }
        let schema = self.get_schema_for_position(&uri, &text, position);
        let target = self.project_sql_symbol_at_position(&uri, &text, position);
        if target.is_none()
            && cte_identifier_range_at_position(&text, position, dialect.name()).is_some()
        {
            let mut locations = dialect.references(&text, position, schema.as_ref()).await;
            let Ok(document_uri) = Url::parse(&uri) else {
                return Ok(None);
            };
            rewrite_current_document_location_uris(&mut locations, &document_uri);
            deduplicate_locations(&mut locations);
            let edits = locations
                .into_iter()
                .map(|location| TextEdit {
                    range: location.range,
                    new_text: params.new_name.clone(),
                })
                .collect::<Vec<_>>();
            if edits.is_empty() {
                return Ok(None);
            }
            return Ok(Some(WorkspaceEdit {
                changes: Some(HashMap::from([(document_uri, edits)])),
                document_changes: None,
                change_annotations: None,
            }));
        }
        let Some(target) = target else {
            return Ok(None);
        };
        if !self.project_sql_symbol_is_renameable(&uri, dialect.name(), schema.as_ref(), &target) {
            return Ok(None);
        }
        let mut changes = HashMap::new();
        for location in self.matching_project_sql_locations(
            &uri,
            dialect.name(),
            schema.as_ref().map(|schema| schema.id),
            &target,
            true,
        ) {
            changes
                .entry(location.uri)
                .or_insert_with(Vec::new)
                .push(TextEdit {
                    range: location.range,
                    new_text: params.new_name.clone(),
                });
        }
        if changes.is_empty() {
            return Ok(None);
        }
        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let uri_string = uri.to_string();
        let text = self.document_manager.get(&uri_string).unwrap_or_default();
        let Some(dialect) = self.get_dialect_for_file(&uri_string) else {
            return Ok(None);
        };
        let dialect_identity = self
            .dialect_identity_for_file(&uri_string)
            .unwrap_or_else(|| dialect.name().to_string());
        let schema = self.get_schema_for_position(&uri_string, &text, params.range.start);
        let mut actions = Vec::new();
        // Formatting is already exposed through textDocument/formatting. Do
        // not format the full console for every automatic lightbulb request;
        // build the source action only when the client explicitly asks for it.
        if code_action_kind_explicitly_requested(&params.context, &CodeActionKind::SOURCE) {
            let formatted = dialect.format(&text).await;
            if formatted != text {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Format document with database dialect".to_string(),
                    kind: Some(CodeActionKind::SOURCE),
                    edit: Some(single_document_edit(
                        uri.clone(),
                        Range::new(Position::new(0, 0), lsp_position_at_end(&text)),
                        formatted,
                    )),
                    ..Default::default()
                }));
            }
        }
        if code_action_kind_available(&params.context, &CodeActionKind::REFACTOR_REWRITE) {
            if let Some(action) = expand_select_star_action(
                &text,
                &uri,
                params.range,
                schema.clone(),
                &dialect_identity,
            ) {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
            actions.extend(
                qualify_identifier_actions(
                    &text,
                    &uri,
                    params.range,
                    schema.as_ref(),
                    &dialect_identity,
                )
                .into_iter()
                .map(CodeActionOrCommand::CodeAction),
            );
        }
        for diagnostic in params
            .context
            .diagnostics
            .iter()
            .filter(|_| code_action_kind_available(&params.context, &CodeActionKind::QUICKFIX))
        {
            if matches!(
                diagnostic.code.as_ref(),
                Some(NumberOrString::String(code)) if code == "OXIDE001"
            ) {
                if let Some(action) = add_mutation_safety_guard_action(&text, &uri, diagnostic) {
                    actions.push(CodeActionOrCommand::CodeAction(action));
                }
            } else if matches!(
                diagnostic.code.as_ref(),
                Some(NumberOrString::String(code)) if code == "OXIDE002"
            ) {
                actions.extend(
                    add_join_condition_actions(
                        &text,
                        &uri,
                        diagnostic,
                        schema.as_ref(),
                        &dialect_identity,
                    )
                    .into_iter()
                    .map(CodeActionOrCommand::CodeAction),
                );
            }
        }
        Ok(Some(actions))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri.to_string();
        let text = self.document_manager.get(&uri).unwrap_or_default();

        if let Some(dialect) = self.get_dialect_for_file(&uri) {
            let preferences = self
                .formatting_preferences
                .read()
                .map(|preferences| preferences.clone())
                .unwrap_or_default();
            let formatted = if dialect_uses_sql_preferences(dialect.name()) {
                let formatted =
                    format_sql_pretty_with_keyword_case(&text, preferences.keyword_uppercase());
                let formatted =
                    apply_formatter_function_case(&formatted, preferences.function_uppercase());
                apply_formatter_layout(
                    &formatted,
                    preferences.logical_operator_layout(),
                    matches!(preferences.from_clause_layout, FromClauseLayout::SameLine),
                )
            } else {
                dialect.format(&text).await
            };
            let range = Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: lsp_position_at_end(&text),
            };
            return Ok(Some(vec![TextEdit {
                range,
                new_text: formatted,
            }]));
        }

        Ok(None)
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri.to_string();
        let text = self.document_manager.get(&uri).unwrap_or_default();
        let Some(dialect) = self.get_dialect_for_file(&uri) else {
            return Ok(None);
        };
        let start = position_to_byte_offset(&text, params.range.start);
        let end = position_to_byte_offset(&text, params.range.end);
        let Some(selected) = text.get(start.min(end)..end.max(start)) else {
            return Ok(None);
        };
        let preferences = self
            .formatting_preferences
            .read()
            .map(|preferences| preferences.clone())
            .unwrap_or_default();
        let formatted = if dialect_uses_sql_preferences(dialect.name()) {
            let formatted =
                format_sql_pretty_with_keyword_case(selected, preferences.keyword_uppercase());
            let formatted =
                apply_formatter_function_case(&formatted, preferences.function_uppercase());
            apply_formatter_layout(
                &formatted,
                preferences.logical_operator_layout(),
                matches!(preferences.from_clause_layout, FromClauseLayout::SameLine),
            )
        } else {
            dialect.format(selected).await
        };
        Ok(Some(vec![TextEdit {
            range: params.range,
            new_text: formatted,
        }]))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri.to_string();
        let text = self.document_manager.get(&uri).unwrap_or_default();
        let project_symbols = self.project_sql_index_for_document(&uri, &text);
        let symbols = document_symbols(
            &text,
            self.get_schema_for_file(&uri).as_ref(),
            &project_symbols,
        );
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let text = self
            .document_manager
            .get(params.text_document.uri.as_str())
            .unwrap_or_default();
        Ok(Some(folding_ranges(&text)))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let uri = params.text_document.uri.to_string();
        let text = self.document_manager.get(&uri).unwrap_or_default();
        let dialect_name = self
            .get_dialect_for_file(&uri)
            .map(|dialect| dialect.name().to_string())
            .unwrap_or_else(|| "postgres".to_string());
        Ok(Some(
            params
                .positions
                .into_iter()
                .map(|position| selection_range_for_position(&text, position, &dialect_name))
                .collect(),
        ))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri.to_string();
        let text = self.document_manager.get(&uri).unwrap_or_default();
        let schema = self.get_schema_for_file(&uri);
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: schema_semantic_tokens(&text, schema.as_ref()),
        })))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri.to_string();
        let text = self.document_manager.get(&uri).unwrap_or_default();
        let Some(schema) = self.get_schema_for_file(&uri) else {
            return Ok(Some(Vec::new()));
        };
        let mut hints = insert_value_hints(&text, params.range, &schema);
        hints.extend(routine_parameter_hints(&text, params.range, &schema));
        hints.sort_by_key(|hint| (hint.position.line, hint.position.character));
        hints.truncate(INLAY_HINT_MAX_ITEMS);
        Ok(Some(hints))
    }

    #[allow(deprecated)]
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let query = params.query.trim().to_ascii_lowercase();
        let mut symbols = Vec::new();
        for schema_id in self.schema_manager.list_ids() {
            let Some(schema) = self.schema_manager.get(schema_id) else {
                continue;
            };
            for table in &schema.tables {
                if workspace_symbol_matches(&query, &table.name, &schema.database) {
                    if let Some(location) = crate::dialects::common::metadata_location(
                        table.source_location.as_ref(),
                        schema.source_uri.as_ref(),
                        "file:///schema.sql",
                    ) {
                        symbols.push(SymbolInformation {
                            name: table.name.clone(),
                            kind: SymbolKind::CLASS,
                            tags: None,
                            deprecated: None,
                            location,
                            container_name: Some(schema.database.clone()),
                        });
                    }
                }
                for column in &table.columns {
                    if !workspace_symbol_matches(&query, &column.name, &table.name) {
                        continue;
                    }
                    if let Some(location) = crate::dialects::common::metadata_location(
                        column
                            .source_location
                            .as_ref()
                            .or(table.source_location.as_ref()),
                        schema.source_uri.as_ref(),
                        "file:///schema.sql",
                    ) {
                        symbols.push(SymbolInformation {
                            name: column.name.clone(),
                            kind: SymbolKind::FIELD,
                            tags: None,
                            deprecated: None,
                            location,
                            container_name: Some(format!("{}.{}", schema.database, table.name)),
                        });
                    }
                }
            }
            for function in &schema.functions {
                if !workspace_symbol_matches(&query, &function.name, &schema.database) {
                    continue;
                }
                if let Some(location) = crate::dialects::common::metadata_location(
                    None,
                    schema.source_uri.as_ref(),
                    "file:///schema.sql",
                ) {
                    symbols.push(SymbolInformation {
                        name: function.name.clone(),
                        kind: SymbolKind::FUNCTION,
                        tags: None,
                        deprecated: None,
                        location,
                        container_name: Some(schema.database.clone()),
                    });
                }
            }
        }
        for (uri, text) in self.project_sql_documents() {
            let Ok(location_uri) = Url::parse(&uri) else {
                continue;
            };
            let container_name = project_sql_symbol_container(&location_uri);
            for occurrence in self.project_sql_index_for_document(&uri, &text) {
                if occurrence.role != ProjectSqlSymbolRole::Definition
                    || !workspace_symbol_matches(
                        &query,
                        &occurrence.name,
                        &format!("{} {}", occurrence.normalized_name, container_name),
                    )
                {
                    continue;
                }
                symbols.push(SymbolInformation {
                    name: occurrence.name,
                    kind: occurrence.kind.symbol_kind(),
                    tags: None,
                    deprecated: None,
                    location: Location {
                        uri: location_uri.clone(),
                        range: occurrence.range,
                    },
                    container_name: Some(container_name.clone()),
                });
                if symbols.len() >= PROJECT_SQL_INDEX_MAX_RESULTS {
                    break;
                }
            }
            if symbols.len() >= PROJECT_SQL_INDEX_MAX_RESULTS {
                break;
            }
        }
        symbols.sort_by(|left, right| {
            workspace_symbol_rank(&query, &left.name)
                .cmp(&workspace_symbol_rank(&query, &right.name))
                .then_with(|| {
                    left.name
                        .to_ascii_lowercase()
                        .cmp(&right.name.to_ascii_lowercase())
                })
                .then_with(|| left.container_name.cmp(&right.container_name))
        });
        symbols.dedup_by(|left, right| {
            left.name.eq_ignore_ascii_case(&right.name)
                && left.location == right.location
                && left.kind == right.kind
        });
        symbols.truncate(1_000);
        Ok(Some(symbols))
    }
}
