//! Current-document completion without configuration, catalog or project reads.
use super::builtin_completion::{
    add_builtin_table_function_completions, add_expression_routine_completions,
};
use super::completion_case::apply_completion_preferences;
use super::completion_context::{
    apply_completed_sql_context_completion_edits, apply_high_frequency_keyword_ranking,
    completion_statement_prefix, deduplicate_simple_completion_items,
};
use super::completion_identifiers::{
    add_referenced_alias_completions, apply_qualified_identifier_completion_edits,
};
use super::document_semantics::local_schema;
use super::preferences::{CompletionPreferences, KeywordCase, TableAliasStyle};
use super::runtime::SqlLspServer;
use std::sync::Arc;
use tower_lsp::{
    jsonrpc::{Error, ErrorCode, Result},
    lsp_types::*,
};

#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentCompletionCase {
    Upper,
    Lower,
    Preserve,
}
impl From<DocumentCompletionCase> for KeywordCase {
    fn from(value: DocumentCompletionCase) -> Self {
        match value {
            DocumentCompletionCase::Upper => Self::Upper,
            DocumentCompletionCase::Lower => Self::Lower,
            DocumentCompletionCase::Preserve => Self::Preserve,
        }
    }
}
#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentCompletionAlias {
    None,
    Initials,
}
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentCompletionPreferences {
    pub keyword_case: DocumentCompletionCase,
    pub function_case: DocumentCompletionCase,
    pub table_alias: DocumentCompletionAlias,
}
impl From<DocumentCompletionPreferences> for CompletionPreferences {
    fn from(value: DocumentCompletionPreferences) -> Self {
        Self {
            keyword_case: value.keyword_case.into(),
            function_case: value.function_case.into(),
            table_alias: match value.table_alias {
                DocumentCompletionAlias::None => TableAliasStyle::None,
                DocumentCompletionAlias::Initials => TableAliasStyle::Initials,
            },
        }
    }
}
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentCompletionParams {
    pub text_document: TextDocumentIdentifier,
    pub version: i32,
    pub dialect: String,
    pub position: Position,
    pub preferences: DocumentCompletionPreferences,
}
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentCompletionSnapshot {
    pub uri: Url,
    pub version: i32,
    pub dialect: String,
    pub position: Position,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraints: Option<super::completion_constraints::CompletionKeywordConstraints>,
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
}
fn modified() -> Error {
    Error {
        code: ErrorCode::ServerError(-32801),
        message: "Document or completion profile changed".into(),
        data: None,
    }
}
impl SqlLspServer {
    pub async fn document_completion_snapshot(
        &self,
        params: DocumentCompletionParams,
    ) -> Result<DocumentCompletionSnapshot> {
        if params.version < 0 {
            return Err(Error::invalid_params("Invalid document version"));
        }
        let profile = self
            .dialect_registry
            .get_explicit_profile(&params.dialect)
            .ok_or_else(|| Error::invalid_params("Unknown completion profile"))?;
        let uri = params.text_document.uri;
        let document = self
            .document_manager
            .snapshot(uri.as_str())
            .ok_or_else(modified)?;
        if document.version != params.version {
            return Err(modified());
        }
        if document.text.len() > super::local_relations::LOCAL_RELATION_SCAN_MAX_BYTES {
            return Err(Error::invalid_params(
                "Document exceeds local completion budget",
            ));
        }
        let text = &document.text;
        let position = params.position;
        if super::completion_context::checked_position_to_byte_offset(text, position).is_none() {
            return Err(Error::invalid_params("Invalid completion position"));
        }
        if let Some(mut candidates) = super::is_predicate_completion::completion_candidates(
            text,
            position,
            &params.dialect,
            || Some(local_schema(text, position, uri.as_str(), profile.name())),
        ) {
            apply_completion_preferences(
                text,
                position,
                &mut candidates.items,
                &params.preferences.into(),
            );
            if self.document_manager.revision(uri.as_str()) != Some(document.revision)
                || self
                    .dialect_registry
                    .get_by_name(&params.dialect)
                    .is_none_or(|current| !Arc::ptr_eq(&current, &profile))
            {
                return Err(modified());
            }
            return Ok(DocumentCompletionSnapshot {
                uri,
                version: document.version,
                dialect: params.dialect,
                position,
                constraints: candidates.constraints,
                value_type: candidates.value_type,
                value_role: candidates.value_role,
                logical_continuation: candidates.logical_continuation,
                pattern_escape: candidates.pattern_escape,
                items: candidates.items,
                is_incomplete: false,
            });
        }
        let schema = local_schema(text, position, uri.as_str(), profile.name());
        let (source, at) = completion_statement_prefix(text, position).unwrap_or((text, position));
        let result = profile
            .completion_with_scope(source, at, Some(&schema), None)
            .await;
        let mut items = result.items;
        let mut incomplete = super::local_relations::add_qualify_completions(
            text,
            position,
            profile.name(),
            Some(&schema),
            uri.as_str(),
            None,
            &mut items,
        );
        if let Some(variables) =
            super::tsql_variable_completion::completions(text, position, profile.name())
        {
            items.extend(variables.items);
            incomplete |= variables.is_incomplete;
        }
        add_expression_routine_completions(
            text,
            position,
            profile.name(),
            Some(&schema),
            &mut items,
            Some((source, at)),
        );
        add_builtin_table_function_completions(source, at, profile.name(), &mut items);
        add_referenced_alias_completions(
            text,
            position,
            &mut items,
            result.relation_aliases.as_deref(),
        );
        apply_completed_sql_context_completion_edits(text, position, &mut items);
        super::completion_quotes::apply_column_identifier_edits(
            text,
            position,
            profile.name(),
            &mut items,
        );
        apply_qualified_identifier_completion_edits(text, position, &mut items);
        let expectation = super::comparison_value_types::enrich(
            text,
            position,
            &params.dialect,
            Some(&schema),
            &mut items,
        );
        let (value_type, value_role) =
            expectation.map_or((None, None), |value| (Some(value.data_type), value.role));
        let continuation = if value_type.is_none() {
            super::logical_continuations::enrich(
                text,
                position,
                &params.dialect,
                Some(&schema),
                &mut items,
            )
        } else {
            None
        };
        let logical_continuation = continuation.map(|knowledge| knowledge.logical);
        let pattern_escape = continuation.and_then(|knowledge| knowledge.pattern_escape);
        apply_completion_preferences(text, position, &mut items, &params.preferences.into());
        deduplicate_simple_completion_items(&mut items);
        apply_high_frequency_keyword_ranking(&mut items);
        let offset = super::completion_context::position_to_byte_offset(text, position);
        let prefix = crate::dialects::common::cursor_prefix_for_dialect(
            crate::placeholder::SqlPlaceholderDialect::for_name(Some(profile.name())),
            text,
            crate::parser::SqlParser::lsp_position_to_byte_position(text, position),
        );
        if offset > 0
            && !super::completion_context::COMPLETED_SQL_CONTEXT_KEYWORDS.contains(&prefix.as_str())
        {
            incomplete |= crate::identifier_search::rank_items(&mut items, &prefix);
            // Metadata search intentionally preserves plain client filtering.
            // Rank plain local names too before this endpoint's wire cap, so
            // an exact late field is not hidden by earlier declarations.
            if !prefix.is_empty() {
                items.sort_by_key(|item| {
                    crate::dialects::common::identifier_match_rank(&item.label, &prefix)
                        .unwrap_or(u8::MAX)
                });
            }
        }
        const MAX_ITEMS: usize = 512;
        incomplete |= items.len() > MAX_ITEMS;
        items.truncate(MAX_ITEMS);
        // These items are complete local values. They must never resolve through
        // the server's catalog cache or carry executable completion commands.
        for item in &mut items {
            item.data = None;
            item.command = None;
            item.additional_text_edits = None;
        }
        // Bound wire work as well as candidate count. Keep ranking order and
        // report truncation, including one individually oversized candidate.
        let mut remaining_bytes = 2 * 1024 * 1024;
        items.retain(|item| {
            let bytes = serde_json::to_vec(item).map_or(usize::MAX, |value| value.len());
            if bytes > remaining_bytes {
                incomplete = true;
                false
            } else {
                remaining_bytes -= bytes;
                true
            }
        });
        if self.document_manager.revision(uri.as_str()) != Some(document.revision)
            || self
                .dialect_registry
                .get_by_name(&params.dialect)
                .is_none_or(|current| !Arc::ptr_eq(&current, &profile))
        {
            return Err(modified());
        }
        Ok(DocumentCompletionSnapshot {
            uri,
            version: document.version,
            dialect: params.dialect,
            position,
            constraints: None,
            value_type,
            value_role,
            logical_continuation,
            pattern_escape,
            items,
            is_incomplete: incomplete,
        })
    }
}
#[cfg(test)]
mod tests;
