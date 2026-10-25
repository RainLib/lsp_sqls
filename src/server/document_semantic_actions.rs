//! Existing action algorithms supplied only an ephemeral document-local shape.
use super::code_actions::{
    add_mutation_safety_guard_action, code_action_kind_explicitly_requested,
    expand_select_star_action, qualify_identifier_actions,
};
use super::join_actions::add_join_condition_actions;
use super::signature_help::{code_action_kind_available, single_document_edit};
use crate::{dialect::Dialect, position::lsp_position_at_end, schema::Schema};
use tower_lsp::lsp_types::*;
pub(super) async fn local_actions(
    profile: &dyn Dialect,
    text: &str,
    uri: &Url,
    range: Range,
    context: &CodeActionContext,
    shape: Schema,
) -> Vec<CodeAction> {
    let mut actions = Vec::new();
    if code_action_kind_explicitly_requested(context, &CodeActionKind::SOURCE) {
        let formatted = profile.format(text).await;
        if formatted != text {
            actions.push(CodeAction {
                title: "Format document with database dialect".into(),
                kind: Some(CodeActionKind::SOURCE),
                edit: Some(single_document_edit(
                    uri.clone(),
                    Range::new(Position::new(0, 0), lsp_position_at_end(text)),
                    formatted,
                )),
                ..Default::default()
            });
        }
    }
    if code_action_kind_available(context, &CodeActionKind::REFACTOR_REWRITE) {
        if let Some(action) =
            expand_select_star_action(text, uri, range, Some(shape.clone()), profile.name())
        {
            actions.push(action);
        }
        actions.extend(qualify_identifier_actions(
            text,
            uri,
            range,
            Some(&shape),
            profile.name(),
        ));
    }
    for diagnostic in context
        .diagnostics
        .iter()
        .filter(|_| code_action_kind_available(context, &CodeActionKind::QUICKFIX))
    {
        match diagnostic.code.as_ref() {
            Some(NumberOrString::String(code)) if code == "OXIDE004" => {
                for mut action in qualify_identifier_actions(
                    text,
                    uri,
                    Range::new(diagnostic.range.end, diagnostic.range.end),
                    Some(&shape),
                    profile.name(),
                ) {
                    if !action.title.starts_with("Qualify column as ") {
                        continue;
                    }
                    actions.retain(|existing| {
                        !(existing.kind == Some(CodeActionKind::REFACTOR_REWRITE)
                            && existing.edit == action.edit)
                    });
                    action.kind = Some(CodeActionKind::QUICKFIX);
                    action.is_preferred = Some(false);
                    action.diagnostics = Some(vec![diagnostic.clone()]);
                    actions.push(action);
                }
            }
            Some(NumberOrString::String(code)) if code == "OXIDE001" => {
                if let Some(action) = add_mutation_safety_guard_action(text, uri, diagnostic) {
                    actions.push(action);
                }
            }
            Some(NumberOrString::String(code)) if code == "OXIDE002" => actions.extend(
                add_join_condition_actions(text, uri, diagnostic, Some(&shape), profile.name()),
            ),
            _ => {}
        }
    }
    actions
}
