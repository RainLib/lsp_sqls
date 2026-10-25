//! Signature formatting, overload selection and editor action helpers.
pub(super) use super::routine_context::{routine_call_at_position, RoutineCallContext};
use super::routine_parameters::RoutineParameters;
use crate::builtin_signatures::BuiltinSignature;
use std::collections::HashMap;
use tower_lsp::lsp_types::*;

pub(super) fn live_overload_accepts_call(
    function: &crate::schema::Function,
    call: &RoutineCallContext,
    dialect: &str,
) -> bool {
    if call.active_group != 0 {
        return false;
    }
    let parameters = RoutineParameters::new(function, dialect, call.procedure_call);
    if call.supplied.has_named() {
        return call.supplied.bind(&parameters, dialect).fits;
    }
    parameters.has_slot(
        call.active_parameter as usize,
        call.current_argument_has_content,
        call.explicit_variadic,
    )
}

pub(super) fn builtin_group_accepts_call(parameters: &[&str], call: &RoutineCallContext) -> bool {
    if parameters.is_empty() {
        return call.active_parameter == 0 && !call.current_argument_has_content;
    }
    call.active_parameter < parameters.len() as u32
        || parameters
            .last()
            .is_some_and(|parameter| parameter.trim_start().starts_with("..."))
}

pub(super) fn builtin_overload_accepts_call(
    signature: &BuiltinSignature,
    call: &RoutineCallContext,
) -> bool {
    signature
        .parameter_groups
        .get(call.active_group as usize)
        .is_some_and(|parameters| builtin_group_accepts_call(parameters, call))
}

pub(super) fn clamped_active_parameter(parameter_count: usize, requested: u32) -> Option<u32> {
    (parameter_count > 0).then(|| requested.min(parameter_count.saturating_sub(1) as u32))
}

pub(super) fn live_signature_information(
    function: &crate::schema::Function,
    call: &RoutineCallContext,
    dialect: &str,
) -> SignatureInformation {
    let parameters = RoutineParameters::new(function, dialect, call.procedure_call);
    SignatureInformation {
        label: format!(
            "{}({})",
            function.name,
            parameters
                .iter()
                .map(|parameter| parameter.signature_label())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: function.markdown_documentation(),
        })),
        parameters: Some(
            parameters
                .iter()
                .map(|parameter| ParameterInformation {
                    label: ParameterLabel::Simple(parameter.signature_label()),
                    documentation: None,
                })
                .collect(),
        ),
        active_parameter: if call.supplied.has_named() {
            let bound = call.supplied.bind(&parameters, dialect);
            bound
                .valid
                .then(|| {
                    bound
                        .slots
                        .get(call.active_parameter as usize)
                        .copied()
                        .flatten()
                })
                .flatten()
                .map(|slot| slot as u32)
        } else {
            clamped_active_parameter(parameters.len(), call.active_parameter)
        },
    }
}

pub(super) fn builtin_signature_information(
    signature: &BuiltinSignature,
    call: &RoutineCallContext,
    dialect: &str,
) -> Option<SignatureInformation> {
    let parameters = signature.parameter_groups.get(call.active_group as usize)?;
    Some(SignatureInformation {
        label: signature.label(),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!(
                "**Built-in signature** for the active `{dialect}` SQL profile. Live routine metadata takes precedence when available."
            ),
        })),
        parameters: Some(
            parameters
                .iter()
                .map(|parameter| ParameterInformation {
                    label: ParameterLabel::Simple((*parameter).to_string()),
                    documentation: None,
                })
                .collect(),
        ),
        active_parameter: clamped_active_parameter(parameters.len(), call.active_parameter),
    })
}

pub(super) fn single_document_edit(uri: Url, range: Range, new_text: String) -> WorkspaceEdit {
    WorkspaceEdit {
        changes: Some(HashMap::from([(uri, vec![TextEdit { range, new_text }])])),
        document_changes: None,
        change_annotations: None,
    }
}

pub(super) fn code_action_kind_available(
    context: &CodeActionContext,
    kind: &CodeActionKind,
) -> bool {
    context.only.as_ref().is_none_or(|only| {
        only.iter().any(|requested| {
            kind.as_str() == requested.as_str()
                || kind
                    .as_str()
                    .starts_with(&format!("{}.", requested.as_str()))
        })
    })
}
