//! Bounded, dialect-aware routine call scanning shared by editor features.
use super::completion_context::position_to_byte_offset;
use super::limits::SIGNATURE_HELP_MAX_SCAN_BYTES;
use super::routine_identity::routine_reference_before;
use crate::parser::SqlParser;
use tower_lsp::lsp_types::Position;

pub(super) fn bounded_routine_prefix(text: &str, position: Position) -> Option<(&str, usize)> {
    let offset = position_to_byte_offset(text, position).min(text.len());
    let mut start = offset.saturating_sub(SIGNATURE_HELP_MAX_SCAN_BYTES);
    while start < offset && !text.is_char_boundary(start) {
        start += 1;
    }
    Some((text.get(start..offset)?, offset))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RoutineCallContext {
    // Original SQL identifier components, including quotes and qualifiers.
    pub(super) name: String,
    pub(super) active_group: u32,
    pub(super) active_parameter: u32,
    pub(super) current_argument_has_content: bool,
    pub(super) arguments: Vec<std::ops::Range<usize>>,
    pub(super) explicit_variadic: bool,
    pub(super) procedure_call: bool,
    pub(super) supplied: super::routine_arguments::RoutineArguments,
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

pub(super) fn routine_name_and_group(
    text: &str,
    masked: &str,
    content: &str,
    mut opening: usize,
    dialect: &str,
) -> Option<(String, u32, bool)> {
    let mut active_group = 0u32;
    loop {
        let (previous, _) = content
            .get(..opening)?
            .char_indices()
            .rev()
            .find(|(_, ch)| !ch.is_whitespace())?;
        let raw_character = text[previous..].chars().next()?;
        if masked.as_bytes().get(previous) == Some(&b')') {
            opening = matching_open_parenthesis(masked, previous)?;
            active_group = active_group.checked_add(1)?;
            if active_group > 3 {
                return None;
            }
            continue;
        }
        let character = raw_character;
        if !character.is_alphanumeric()
            && !matches!(character, '_' | '$' | '@' | '#' | '"' | '`' | '[')
        {
            return None;
        }
        return routine_reference_before(text, content, opening, dialect)
            .map(|(name, procedure)| (name, active_group, procedure));
    }
}

pub(super) fn routine_call_at_position(
    text: &str,
    position: Position,
    dialect: &str,
) -> Option<RoutineCallContext> {
    let (before, _) = bounded_routine_prefix(text, position)?;
    let masked = SqlParser::mask_sql_noise_for_dialect(before, dialect);
    let content = SqlParser::mask_sql_operand_context_for_dialect(before, dialect);
    let mut nesting = 0u32;
    let mut opening = None;
    for (index, character) in masked.char_indices().rev() {
        match character {
            ')' => nesting += 1,
            '(' if nesting > 0 => nesting -= 1,
            '(' => {
                if let Some((name, group, procedure)) =
                    routine_name_and_group(before, &masked, &content, index, dialect)
                {
                    opening = Some((index, name, group, procedure));
                    break;
                }
            }
            _ => {}
        }
    }
    let Some((opening, name, active_group, procedure_call)) = opening else {
        return super::execute_context::call_at_position(text, position, dialect);
    };

    let mut active_parameter = 0u32;
    let mut nested = 0u32;
    let mut current_argument_start = opening + 1;
    let absolute = position_to_byte_offset(text, position).min(text.len()) - before.len();
    let mut arguments = Vec::new();
    let mut arguments_over_budget = false;
    for (relative, character) in masked[opening + 1..].char_indices() {
        match character {
            '(' | '[' | '{' => nested += 1,
            ')' | ']' | '}' if nested > 0 => nested -= 1,
            ',' if nested == 0 => {
                if arguments.len() >= 256 {
                    arguments.clear();
                    arguments_over_budget = true;
                }
                if !arguments_over_budget {
                    arguments
                        .push(absolute + current_argument_start..absolute + opening + 1 + relative);
                }
                active_parameter += 1;
                current_argument_start = opening + 1 + relative + character.len_utf8();
            }
            ';' if nested == 0 => return None,
            _ => {}
        }
    }
    let current_argument_has_content = content[current_argument_start..]
        .chars()
        .any(|ch| !ch.is_whitespace());
    if !arguments_over_budget && arguments.len() < 256 {
        arguments.push(absolute + current_argument_start..absolute + before.len());
    } else {
        arguments.clear();
    }
    Some(RoutineCallContext {
        supplied: super::routine_arguments::complete_arguments(text, absolute + opening, dialect)
            .unwrap_or_else(|| {
                super::routine_arguments::RoutineArguments::from_ranges(
                    text, &arguments, false, dialect,
                )
            }),
        explicit_variadic: arguments.iter().any(|range| {
            super::routine_parameters::explicit_variadic(
                &content[range.start - absolute..range.end - absolute],
            )
        }),
        name,
        active_group,
        active_parameter,
        current_argument_has_content,
        arguments,
        procedure_call,
    })
}
