//! Routine hints share qualified identity and dialect-aware lexical masks.
use super::completion_context::position_to_byte_offset;
use super::document_structure::position_for_offset;
use super::limits::SIGNATURE_HELP_MAX_SCAN_BYTES;
use super::routine_arguments::complete_arguments_in_masks;
use super::routine_identity::{routine_reference_before, RoutineIdentity};
use super::routine_parameters::{routine_kind_matches, RoutineParameters};
use crate::parser::SqlParser;
use crate::schema::{Schema, SchemaManager};
use std::collections::HashSet;
use tower_lsp::lsp_types::*;

enum Arguments {
    Parenthesized(usize),
    Execute(super::routine_arguments::RoutineArguments),
}

pub(super) fn routine_parameter_hints(
    text: &str,
    range: Range,
    schema: &Schema,
    manager: &SchemaManager,
    dialect: &str,
) -> Vec<InlayHint> {
    let range_start = position_to_byte_offset(text, range.start);
    let range_end = position_to_byte_offset(text, range.end);
    let masked = SqlParser::mask_sql_noise_for_dialect(text, dialect);
    let content = SqlParser::mask_sql_operand_context_for_dialect(text, dialect);
    let mut hints = Vec::new();
    let calls = masked.char_indices().filter_map(|(opening, character)| {
        if character != '(' || opening > range_end {
            return None;
        }
        let mut start = opening.saturating_sub(SIGNATURE_HELP_MAX_SCAN_BYTES);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        let (reference, procedure) = routine_reference_before(
            &text[start..opening],
            &content[start..opening],
            opening - start,
            dialect,
        )?;
        Some((reference, procedure, Arguments::Parenthesized(opening)))
    });
    let execute =
        super::execute_context::calls_in_masks(text, &masked, &content, range_end, dialect)
            .map(|call| (call.name, true, Arguments::Execute(call.supplied)));
    for (reference, procedure, arguments) in calls.chain(execute) {
        let Some(identity) = RoutineIdentity::parse(&reference, dialect) else {
            continue;
        };
        let Some(metadata) = identity.schema(Some(schema), manager, dialect) else {
            continue;
        };
        let overloads = metadata
            .functions
            .iter()
            .filter(|function| {
                identity.matches(function, dialect) && routine_kind_matches(function, procedure)
            })
            .collect::<Vec<_>>();
        if overloads.is_empty() {
            continue;
        }
        // Unknown calls never pay for argument-list extraction. Both call
        // forms are streamed and reuse this request's lexical masks.
        let arguments = match arguments {
            Arguments::Parenthesized(opening) => {
                let Some(arguments) =
                    complete_arguments_in_masks(text, &masked, &content, opening, dialect)
                else {
                    continue;
                };
                arguments
            }
            Arguments::Execute(arguments) => arguments,
        };
        let explicit = arguments.values.iter().any(|argument| argument.explicit);
        let matching_overloads = overloads
            .into_iter()
            .map(|function| RoutineParameters::new(function, dialect, procedure))
            .filter_map(|parameters| {
                let binding = arguments.bind(&parameters, dialect);
                binding.fits.then_some((parameters, binding))
            })
            .collect::<Vec<_>>();
        if matching_overloads.is_empty() {
            continue;
        }
        for (parameter_index, argument) in arguments.values.iter().enumerate() {
            if argument.name.is_some() {
                continue;
            }
            let argument_start = argument.full.start
                + content[argument.full.clone()]
                    .find(|ch: char| !ch.is_whitespace())
                    .unwrap_or(argument.full.len());
            if argument_start < range_start || argument_start > range_end {
                continue;
            }
            let names = matching_overloads
                .iter()
                .filter_map(|(parameters, binding)| {
                    parameters.parameter(binding.slots.get(parameter_index).copied().flatten()?)
                })
                .map(|parameter| parameter.name.trim())
                .filter(|name| !name.is_empty())
                .collect::<HashSet<_>>();
            if names.len() != 1 {
                continue;
            }
            let name = *names.iter().next().expect("one parameter name");
            if text[argument_start..]
                .strip_prefix(name)
                .is_some_and(|tail| {
                    tail.trim_start().starts_with("=>") || tail.trim_start().starts_with(":=")
                })
            {
                continue;
            }
            let data_types = matching_overloads
                .iter()
                .filter_map(|(parameters, binding)| {
                    parameters.data_type(
                        binding.slots.get(parameter_index).copied().flatten()?,
                        explicit,
                    )
                })
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
        if hints.len() >= 500 {
            break;
        }
    }
    hints.sort_by_key(|hint| (hint.position.line, hint.position.character));
    hints.truncate(500);
    hints
}
