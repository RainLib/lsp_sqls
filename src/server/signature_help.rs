//! Signature help for the SQL language server.
use super::completion_context::position_to_byte_offset;
use super::completion_identifiers::identifier_path_start_before_cursor;
use super::limits::SIGNATURE_HELP_MAX_SCAN_BYTES;
use crate::builtin_signatures::BuiltinSignature;
use crate::parser::SqlParser;
use std::collections::HashMap;
use tower_lsp::lsp_types::*;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RoutineCallContext {
    pub(super) name: String,
    pub(super) active_group: u32,
    pub(super) active_parameter: u32,
    pub(super) current_argument_has_content: bool,
}

pub(super) fn previous_char_start(text: &str, end: usize) -> Option<usize> {
    text.get(..end)?
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
}

pub(super) fn matching_open_parenthesis(masked: &str, close: usize) -> Option<usize> {
    let mut nesting = 0u32;
    for (index, character) in masked.get(..=close)?.char_indices().rev() {
        match character {
            ')' => nesting += 1,
            '(' => {
                nesting = nesting.checked_sub(1)?;
                if nesting == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn routine_name_before(text: &str, end: usize) -> Option<String> {
    let mut name_end = end.min(text.len());
    while name_end > 0
        && text[..name_end]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
    {
        name_end = previous_char_start(text, name_end)?;
    }
    let token_start = identifier_path_start_before_cursor(text.get(..name_end)?)?;
    let raw = text.get(token_start..name_end)?.trim();
    if raw.is_empty() || raw.starts_with('\'') {
        return None;
    }
    let name = SqlParser::identifier_last_part(raw);
    (!name.is_empty()).then_some(name)
}

pub(super) fn routine_name_and_group(
    text: &str,
    masked: &str,
    mut opening: usize,
) -> Option<(String, u32)> {
    let mut active_group = 0u32;
    loop {
        let mut before = opening;
        while before > 0
            && text[..before]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            before = previous_char_start(text, before)?;
        }
        let previous = previous_char_start(text, before)?;
        if masked.as_bytes().get(previous) == Some(&b')') {
            opening = matching_open_parenthesis(masked, previous)?;
            active_group = active_group.checked_add(1)?;
            if active_group > 3 {
                return None;
            }
            continue;
        }
        return routine_name_before(text, before).map(|name| (name, active_group));
    }
}

pub(super) fn routine_call_at_position(
    text: &str,
    position: Position,
) -> Option<RoutineCallContext> {
    let offset = position_to_byte_offset(text, position).min(text.len());
    let mut scan_start = offset.saturating_sub(SIGNATURE_HELP_MAX_SCAN_BYTES);
    while scan_start < offset && !text.is_char_boundary(scan_start) {
        scan_start += 1;
    }
    let before = text.get(scan_start..offset)?;
    let masked = SqlParser::mask_sql_noise(before);
    let mut nesting = 0u32;
    let mut opening = None;
    for (index, character) in masked.char_indices().rev() {
        match character {
            ')' => nesting += 1,
            '(' if nesting > 0 => nesting -= 1,
            '(' => {
                opening = Some(index);
                break;
            }
            _ => {}
        }
    }
    let opening = opening?;
    let (name, active_group) = routine_name_and_group(before, &masked, opening)?;

    let mut active_parameter = 0u32;
    let mut nested = 0u32;
    let mut current_argument_start = opening + 1;
    for (relative, character) in masked[opening + 1..].char_indices() {
        match character {
            '(' | '[' | '{' => nested += 1,
            ')' | ']' | '}' if nested > 0 => nested -= 1,
            ',' if nested == 0 => {
                active_parameter += 1;
                current_argument_start = opening + 1 + relative + character.len_utf8();
            }
            ';' if nested == 0 => return None,
            _ => {}
        }
    }
    let current_argument_has_content = sql_fragment_has_content(&before[current_argument_start..]);
    Some(RoutineCallContext {
        name,
        active_group,
        active_parameter,
        current_argument_has_content,
    })
}

pub(super) fn sql_fragment_has_content(fragment: &str) -> bool {
    let mut offset = 0usize;
    while offset < fragment.len() {
        let rest = &fragment[offset..];
        let Some(character) = rest.chars().next() else {
            return false;
        };
        if character.is_whitespace() {
            offset += character.len_utf8();
            continue;
        }
        if rest.starts_with("--")
            || (rest.starts_with('#') && !rest.starts_with("#>") && !rest.starts_with("#-"))
        {
            let Some(newline) = rest.find('\n') else {
                return false;
            };
            offset += newline + 1;
            continue;
        }
        if rest.starts_with("/*") {
            let Some(end) = rest.get(2..).and_then(|tail| tail.find("*/")) else {
                return false;
            };
            offset += 2 + end + 2;
            continue;
        }
        return true;
    }
    false
}

pub(super) fn routine_names_match(candidate: &str, requested: &str) -> bool {
    SqlParser::identifier_last_part(candidate).eq_ignore_ascii_case(requested)
}

pub(super) fn live_overload_accepts_call(
    function: &crate::schema::Function,
    call: &RoutineCallContext,
) -> bool {
    if call.active_group != 0 {
        return false;
    }
    let parameters = &function.parameters;
    if parameters.is_empty() {
        return call.active_parameter == 0 && !call.current_argument_has_content;
    }
    call.active_parameter < parameters.len() as u32
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
) -> SignatureInformation {
    SignatureInformation {
        label: function.signature(),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: function.markdown_documentation(),
        })),
        parameters: Some(
            function
                .parameters
                .iter()
                .map(|parameter| ParameterInformation {
                    label: ParameterLabel::Simple(parameter.signature_label()),
                    documentation: None,
                })
                .collect(),
        ),
        active_parameter: clamped_active_parameter(
            function.parameters.len(),
            call.active_parameter,
        ),
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
