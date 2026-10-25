//! Revision-bound local semantics. No configuration, catalog or project reads.
use super::document_structure::position_for_offset;
use super::insert_hints::insert_value_hints_with_metadata;
use super::limits::{
    INLAY_HINT_MAX_ITEMS, PROJECT_SQL_INDEX_MAX_RESULTS, SIGNATURE_HELP_MAX_OVERLOADS,
};
use super::local_relations::augment_schema_with_local_relations_and_routines;
use super::locations::{
    deduplicate_locations, range_contains_position, rewrite_current_document_location_uri,
    rewrite_current_document_location_uris, supports_semantic_rename, valid_renamed_identifier,
};
use super::project_index::{project_sql_symbol_occurrences, project_sql_symbols_match};
use super::project_types::{ProjectSqlSymbolOccurrence, ProjectSqlSymbolRole};
use super::routine_identity::RoutineIdentity;
use super::runtime::SqlLspServer;
use super::semantic_tokens::schema_semantic_tokens;
use super::signature_help::{
    builtin_overload_accepts_call, builtin_signature_information, routine_call_at_position,
};
use crate::{
    builtin_signatures::builtin_signatures_for,
    dialect::Dialect,
    schema::{Schema, SchemaId},
};
use std::{collections::HashMap, sync::Arc};
use tower_lsp::{
    jsonrpc::{Error, ErrorCode, Result},
    lsp_types::*,
};

#[derive(Debug, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DocumentSemanticOperation {
    Hover {
        position: Position,
    },
    Definitions {
        position: Position,
    },
    References {
        position: Position,
        include_declaration: bool,
    },
    Signature {
        position: Position,
    },
    PrepareRename {
        position: Position,
    },
    Rename {
        position: Position,
        new_name: String,
    },
    CodeActions {
        range: Range,
        context: CodeActionContext,
    },
    Tokens {},
    InlayHints {
        range: Range,
    },
}
impl DocumentSemanticOperation {
    fn kind(&self) -> &'static str {
        match self {
            Self::Hover { .. } => "hover",
            Self::Definitions { .. } => "definitions",
            Self::References { .. } => "references",
            Self::Signature { .. } => "signature",
            Self::PrepareRename { .. } => "prepareRename",
            Self::Rename { .. } => "rename",
            Self::CodeActions { .. } => "codeActions",
            Self::Tokens {} => "tokens",
            Self::InlayHints { .. } => "inlayHints",
        }
    }
    fn position(&self, text: &str) -> Position {
        match self {
            Self::Hover { position }
            | Self::Definitions { position }
            | Self::References { position, .. }
            | Self::Signature { position }
            | Self::PrepareRename { position }
            | Self::Rename { position, .. } => *position,
            Self::CodeActions { range, .. } | Self::InlayHints { range } => range.start,
            Self::Tokens {} => position_for_offset(text, text.len()),
        }
    }
}
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentSemanticParams {
    pub text_document: TextDocumentIdentifier,
    pub version: i32,
    pub dialect: String,
    pub operation: DocumentSemanticOperation,
}
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSemanticSnapshot {
    pub uri: Url,
    pub version: i32,
    pub dialect: String,
    pub kind: &'static str,
    pub value: serde_json::Value,
}
fn modified() -> Error {
    Error {
        code: ErrorCode::ServerError(-32801),
        message: "Document or semantic profile changed".into(),
        data: None,
    }
}
pub(super) fn local_schema(text: &str, position: Position, uri: &str, dialect: &str) -> Schema {
    augment_schema_with_local_relations_and_routines(
        Schema {
            id: SchemaId(uuid::Uuid::nil()),
            server: None,
            namespace: None,
            catalog: None,
            database: String::new(),
            server_version: None,
            tables: Vec::new(),
            functions: Vec::new(),
            source_uri: Some(uri.into()),
        },
        text,
        position,
        uri,
        Some(dialect),
        None,
    )
}
fn builtin_help(text: &str, position: Position, dialect: &str) -> Option<SignatureHelp> {
    let call = routine_call_at_position(text, position, dialect)?;
    let identity = RoutineIdentity::parse(&call.name, dialect)?;
    if call.procedure_call || call.supplied.has_named() || !identity.permits_builtin(dialect) {
        return None;
    }
    let mut builtins = builtin_signatures_for(dialect, identity.name(), None);
    builtins.retain(|signature| identity.matches_builtin(&signature.name, dialect));
    builtins.sort_by_key(|signature| !builtin_overload_accepts_call(signature, &call));
    builtins.truncate(SIGNATURE_HELP_MAX_OVERLOADS);
    let signatures = builtins
        .iter()
        .filter_map(|signature| builtin_signature_information(signature, &call, dialect))
        .collect::<Vec<_>>();
    (!signatures.is_empty()).then(|| SignatureHelp {
        active_parameter: signatures[0].active_parameter,
        signatures,
        active_signature: Some(0),
    })
}
fn local_occurrences(
    text: &str,
    position: Position,
) -> (
    Vec<ProjectSqlSymbolOccurrence>,
    Option<ProjectSqlSymbolOccurrence>,
) {
    let occurrences = project_sql_symbol_occurrences(text);
    let target = occurrences
        .iter()
        .find(|item| range_contains_position(item.range, position))
        .cloned();
    (occurrences, target)
}

fn local_column_definition(
    text: &str,
    position: Position,
    shape: &Schema,
    uri: &Url,
) -> Option<Location> {
    let mut parser = crate::parser::SqlParser::new();
    let tree = parser.parse_syntax_tree(text)?;
    let point = crate::parser::SqlParser::lsp_position_to_byte_position(text, position);
    let node = parser.get_node_at_position(&tree, point)?;
    let (_, column) =
        crate::dialects::common::column_at_position(&parser, &tree, node, text, point, shape)?;
    let location = crate::dialects::common::metadata_location(
        column.source_location.as_ref(),
        shape.source_uri.as_ref(),
        uri.as_str(),
    )?;
    (location.uri == *uri).then_some(location)
}
async fn local_locations(
    profile: &dyn Dialect,
    text: &str,
    uri: &Url,
    position: Position,
    include_declaration: bool,
) -> Vec<Location> {
    if let Some(resolved) = super::document_cte_locations::resolve_cte(
        text,
        uri,
        position,
        profile.name(),
        include_declaration,
    ) {
        return resolved.locations;
    }
    let (occurrences, target) = local_occurrences(text, position);
    let mut locations = if let Some(target) = target {
        occurrences
            .into_iter()
            .filter(|item| {
                (include_declaration || item.role != ProjectSqlSymbolRole::Definition)
                    && project_sql_symbols_match(&target, item)
            })
            .map(|item| Location {
                uri: uri.clone(),
                range: item.range,
            })
            .collect()
    } else {
        let shape = local_schema(text, position, uri.as_str(), profile.name());
        let mut items = profile.references(text, position, Some(&shape)).await;
        rewrite_current_document_location_uris(&mut items, uri);
        items
    };
    locations.retain(|location| location.uri == *uri);
    deduplicate_locations(&mut locations);
    locations.truncate(PROJECT_SQL_INDEX_MAX_RESULTS);
    locations
}
async fn rename_range(
    profile: &dyn Dialect,
    text: &str,
    uri: &Url,
    position: Position,
) -> Option<Range> {
    if !matches!(
        profile.name(),
        "postgres" | "mysql" | "sqlite" | "duckdb" | "sqlserver" | "hive" | "clickhouse"
    ) {
        return None;
    }
    if let Some(resolved) =
        super::document_cte_locations::resolve_cte(text, uri, position, profile.name(), true)
    {
        return Some(resolved.selected);
    }
    if !supports_semantic_rename(profile.name()) {
        return None;
    }
    let (occurrences, target) = local_occurrences(text, position);
    if let Some(target) = target {
        return (occurrences
            .iter()
            .filter(|item| {
                item.role == ProjectSqlSymbolRole::Definition
                    && project_sql_symbols_match(&target, item)
            })
            .count()
            == 1)
            .then_some(target.range);
    }
    None
}
impl SqlLspServer {
    pub async fn document_semantic_snapshot(
        &self,
        params: DocumentSemanticParams,
    ) -> Result<DocumentSemanticSnapshot> {
        if params.version < 0 {
            return Err(Error::invalid_params("Invalid document version"));
        }
        let profile = self
            .dialect_registry
            .get_explicit_profile(&params.dialect)
            .ok_or_else(|| Error::invalid_params("Unknown semantic profile"))?;
        let uri = &params.text_document.uri;
        let snapshot = self
            .document_manager
            .snapshot(uri.as_str())
            .ok_or_else(modified)?;
        if snapshot.version != params.version {
            return Err(modified());
        }
        let text = &snapshot.text;
        let operation = &params.operation;
        if matches!(operation,DocumentSemanticOperation::CodeActions{range,..}|DocumentSemanticOperation::InlayHints{range} if range.start>range.end)
        {
            return Err(Error::invalid_params("Invalid semantic range"));
        }
        let position = operation.position(text);
        let shape = || local_schema(text, position, uri.as_str(), profile.name());
        let value = match operation {
            DocumentSemanticOperation::Hover { .. } => {
                let shape = shape();
                serde_json::to_value(profile.hover(text, position, Some(&shape)).await)
            }
            DocumentSemanticOperation::Definitions { .. } => {
                let shape = shape();
                let mut locations = Vec::new();
                if let Some(resolved) = super::document_cte_locations::resolve_cte(
                    text,
                    uri,
                    position,
                    profile.name(),
                    true,
                ) {
                    locations.push(Location {
                        uri: uri.clone(),
                        range: resolved.definition,
                    });
                }
                if locations.is_empty() {
                    if let Some(mut location) =
                        profile.goto_definition(text, position, Some(&shape)).await
                    {
                        rewrite_current_document_location_uri(&mut location, uri);
                        if location.uri == *uri {
                            locations.push(location);
                        }
                    }
                }
                if locations.is_empty() {
                    if let Some(location) = local_column_definition(text, position, &shape, uri) {
                        locations.push(location);
                    }
                }
                if locations.is_empty() {
                    let (items, target) = local_occurrences(text, position);
                    if let Some(target) = target {
                        if let Some(item) = items.into_iter().find(|item| {
                            item.role == ProjectSqlSymbolRole::Definition
                                && project_sql_symbols_match(&target, item)
                        }) {
                            locations.push(Location {
                                uri: uri.clone(),
                                range: item.range,
                            });
                        }
                    }
                }
                serde_json::to_value(locations)
            }
            DocumentSemanticOperation::References {
                include_declaration,
                ..
            } => serde_json::to_value(
                local_locations(&*profile, text, uri, position, *include_declaration).await,
            ),
            DocumentSemanticOperation::Signature { .. } => {
                serde_json::to_value(builtin_help(text, position, profile.name()))
            }
            DocumentSemanticOperation::PrepareRename { .. } => {
                serde_json::to_value(rename_range(&*profile, text, uri, position).await)
            }
            DocumentSemanticOperation::Rename { new_name, .. } => {
                let edit = if valid_renamed_identifier(new_name)
                    && rename_range(&*profile, text, uri, position).await.is_some()
                {
                    let edits = local_locations(&*profile, text, uri, position, true)
                        .await
                        .into_iter()
                        .map(|location| TextEdit {
                            range: location.range,
                            new_text: new_name.clone(),
                        })
                        .collect::<Vec<_>>();
                    (!edits.is_empty()).then(|| WorkspaceEdit {
                        changes: Some(HashMap::from([(uri.clone(), edits)])),
                        document_changes: None,
                        change_annotations: None,
                    })
                } else {
                    None
                };
                serde_json::to_value(edit)
            }
            DocumentSemanticOperation::CodeActions { range, context } => serde_json::to_value(
                super::document_semantic_actions::local_actions(
                    &*profile,
                    text,
                    uri,
                    *range,
                    context,
                    shape(),
                )
                .await,
            ),
            DocumentSemanticOperation::Tokens {} => serde_json::to_value(SemanticTokens {
                result_id: None,
                data: schema_semantic_tokens(text, Some(&shape())),
            }),
            DocumentSemanticOperation::InlayHints { range } => {
                let shape = shape();
                let mut hints = insert_value_hints_with_metadata(text, *range, Some(&shape), None);
                hints.sort_by_key(|hint| (hint.position.line, hint.position.character));
                hints.truncate(INLAY_HINT_MAX_ITEMS);
                serde_json::to_value(hints)
            }
        }
        .map_err(|_| Error::internal_error())?;
        if self.document_manager.revision(uri.as_str()) != Some(snapshot.revision)
            || self
                .dialect_registry
                .get_by_name(&params.dialect)
                .is_none_or(|current| !Arc::ptr_eq(&current, &profile))
        {
            return Err(modified());
        }
        Ok(DocumentSemanticSnapshot {
            uri: uri.clone(),
            version: snapshot.version,
            dialect: params.dialect,
            kind: operation.kind(),
            value,
        })
    }
}

#[cfg(test)]
mod tests;
