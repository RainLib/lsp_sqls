//! Routine hints for the SQL language server.
use super::completion_context::position_to_byte_offset;
use super::document_structure::position_for_offset;
use super::semantic_tokens::sql_identifier_tokens;
use super::sql_scan::skip_sql_whitespace;
use crate::schema::Schema;
use std::collections::HashSet;
use tower_lsp::lsp_types::*;
pub(super) fn routine_parameter_hints(text: &str, range: Range, schema: &Schema) -> Vec<InlayHint> {
    let range_start = position_to_byte_offset(text, range.start);
    let range_end = position_to_byte_offset(text, range.end);
    let mut hints = Vec::new();

    for (identifier, _, name_end) in sql_identifier_tokens(text) {
        let mut opening = name_end;
        while text[opening..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
        {
            let Some(character) = text[opening..].chars().next() else {
                break;
            };
            opening += character.len_utf8();
        }
        if !text[opening..].starts_with('(') {
            continue;
        }
        let overloads = schema
            .functions
            .iter()
            .filter(|function| function.name.eq_ignore_ascii_case(&identifier))
            .collect::<Vec<_>>();
        if overloads.is_empty() {
            continue;
        }
        let Some(arguments) = routine_argument_offsets(text, opening) else {
            continue;
        };
        let matching_overloads = overloads
            .into_iter()
            .filter(|function| {
                let required = function
                    .parameters
                    .iter()
                    .filter(|parameter| !parameter.optional)
                    .count();
                required <= arguments.len() && arguments.len() <= function.parameters.len()
            })
            .collect::<Vec<_>>();
        if matching_overloads.is_empty() {
            continue;
        }

        for (parameter_index, argument_start) in arguments.into_iter().enumerate() {
            if argument_start < range_start || argument_start > range_end {
                continue;
            }
            let names = matching_overloads
                .iter()
                .filter_map(|function| function.parameters.get(parameter_index))
                .map(|parameter| parameter.name.trim())
                .filter(|name| !name.is_empty())
                .collect::<HashSet<_>>();
            if names.len() != 1 {
                continue;
            }
            let name = *names.iter().next().expect("one parameter name");
            let argument_tail = &text[argument_start..];
            if argument_tail.strip_prefix(name).is_some_and(|tail| {
                tail.trim_start().starts_with("=>") || tail.trim_start().starts_with(":=")
            }) {
                continue;
            }
            let data_types = matching_overloads
                .iter()
                .filter_map(|function| function.parameters.get(parameter_index))
                .map(|parameter| parameter.data_type.trim())
                .filter(|data_type| !data_type.is_empty())
                .collect::<HashSet<_>>();
            let tooltip = (data_types.len() == 1).then(|| {
                InlayHintTooltip::String(format!(
                    "Parameter type: {}",
                    data_types.iter().next().expect("one data type")
                ))
            });
            hints.push(InlayHint {
                position: position_for_offset(text, argument_start),
                label: InlayHintLabel::String(format!("{name}:")),
                kind: Some(InlayHintKind::PARAMETER),
                text_edits: None,
                tooltip,
                padding_left: None,
                padding_right: Some(true),
                data: None,
            });
        }
    }
    hints.sort_by_key(|hint| (hint.position.line, hint.position.character));
    hints.truncate(500);
    hints
}

pub(super) fn routine_argument_offsets(text: &str, opening: usize) -> Option<Vec<usize>> {
    let mut arguments = Vec::new();
    let mut nesting = 0u32;
    let mut quote = None;
    let mut argument_start = opening + 1;
    let mut saw_content = false;

    for (relative, character) in text[opening + 1..].char_indices() {
        let offset = opening + 1 + relative;
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            }
            saw_content = true;
            continue;
        }
        if matches!(character, '\'' | '"' | '`') {
            quote = Some(character);
            saw_content = true;
            continue;
        }
        match character {
            '(' | '[' | '{' => {
                nesting += 1;
                saw_content = true;
            }
            ')' if nesting == 0 => {
                if saw_content {
                    arguments.push(skip_sql_whitespace(text, argument_start, offset));
                }
                return Some(arguments);
            }
            ')' | ']' | '}' if nesting > 0 => {
                nesting -= 1;
                saw_content = true;
            }
            ',' if nesting == 0 => {
                arguments.push(skip_sql_whitespace(text, argument_start, offset));
                argument_start = offset + character.len_utf8();
                saw_content = false;
            }
            _ if !character.is_whitespace() => saw_content = true,
            _ => {}
        }
    }
    None
}
