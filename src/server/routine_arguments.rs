//! One lexical argument/binding view for live signatures, hints and name menus.
use super::limits::SIGNATURE_HELP_MAX_SCAN_BYTES;
use super::routine_identity::read_parameter_name;
use super::routine_parameters::{explicit_variadic, RoutineParameters};
use crate::{
    parser::SqlParser, placeholder::SqlPlaceholderDialect, sql_lexical::lexical_rules_for,
};
use std::{collections::HashSet, ops::Range};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Argument {
    pub(super) name: Option<String>,
    pub(super) value: Range<usize>,
    pub(super) full: Range<usize>,
    pub(super) explicit: bool,
    pub(super) has_value: bool,
}
pub(super) fn argument(text: &str, range: Range<usize>, dialect: &str) -> Argument {
    let raw = &text[range.clone()];
    let content = SqlParser::mask_sql_operand_context_for_dialect(raw, dialect);
    let mut start = content
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(raw.len());
    let explicit = explicit_variadic(&content[start..]);
    if explicit {
        start += 8;
        start += content[start..]
            .find(|ch: char| !ch.is_whitespace())
            .unwrap_or(content.len() - start);
    }
    let named = read_parameter_name(&raw[start..], dialect).and_then(|(name, end)| {
        let after = start + end;
        let operator = after
            + content[after..]
                .find(|ch: char| !ch.is_whitespace())
                .unwrap_or(content.len() - after);
        if lexical_rules_for(dialect).placeholder == SqlPlaceholderDialect::SqlServer {
            (name.starts_with('@')
                && content[operator..].starts_with('=')
                && !content[operator..].starts_with("=="))
            .then_some((name, operator + 1))
        } else {
            (content[operator..].starts_with("=>") || content[operator..].starts_with(":="))
                .then_some((name, operator + 2))
        }
    });
    let name = named.as_ref().map(|(name, _)| name.clone());
    if let Some((_, value)) = named {
        start = value;
    }
    start += content[start..]
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(content.len() - start);
    Argument {
        name,
        value: range.start + start..range.end,
        full: range,
        explicit,
        has_value: start < content.len(),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct RoutineArguments {
    pub(super) values: Vec<Argument>,
    pub(super) complete: bool,
}
pub(super) struct Binding {
    pub(super) slots: Vec<Option<usize>>,
    pub(super) valid: bool,
    pub(super) fits: bool,
    pub(super) used: HashSet<usize>,
}
impl RoutineArguments {
    pub(super) fn from_ranges(
        text: &str,
        ranges: &[Range<usize>],
        complete: bool,
        dialect: &str,
    ) -> Self {
        Self {
            values: ranges
                .iter()
                .map(|range| argument(text, range.clone(), dialect))
                .collect(),
            complete,
        }
    }
    pub(super) fn has_named(&self) -> bool {
        self.values.iter().any(|argument| argument.name.is_some())
    }
    pub(super) fn bind(&self, parameters: &RoutineParameters<'_>, dialect: &str) -> Binding {
        self.bind_inner(parameters, dialect, None)
    }
    pub(super) fn available_named(
        &self,
        parameters: &RoutineParameters<'_>,
        dialect: &str,
        index: usize,
    ) -> Binding {
        self.bind_inner(parameters, dialect, Some(index))
    }
    fn bind_inner(
        &self,
        parameters: &RoutineParameters<'_>,
        dialect: &str,
        editing_named: Option<usize>,
    ) -> Binding {
        let names_supported = matches!(
            lexical_rules_for(dialect).placeholder,
            SqlPlaceholderDialect::Postgres | SqlPlaceholderDialect::SqlServer
        );
        let mut used = HashSet::new();
        let mut named = false;
        let mut valid = true;
        let mut supplied = true;
        let mut slots = Vec::with_capacity(self.values.len());
        for (index, arg) in self.values.iter().enumerate() {
            if editing_named == Some(index) {
                named = true;
                slots.push(None);
                continue;
            }
            let name = arg.name.as_deref();
            let slot = if let Some(name) = name {
                named = true;
                if !names_supported {
                    valid = false;
                }
                parameters.named_index(name)
            } else if named && arg.has_value {
                valid = false;
                None
            } else if named && !arg.has_value {
                None // An empty editor slot after named arguments is not positional SQL.
            } else {
                parameters.declaration_index(index)
            };
            if let Some(slot) = slot {
                let repeated =
                    name.is_none() && !arg.explicit && parameters.variadic_index() == Some(slot);
                if !used.insert(slot) && !repeated {
                    valid = false;
                }
                if name.is_some() && parameters.variadic_index() == Some(slot) && !arg.explicit {
                    valid = false;
                }
                if arg.explicit
                    && (parameters.variadic_index() != Some(slot) || index + 1 != self.values.len())
                {
                    valid = false;
                }
            } else if arg.has_value || name.is_some() {
                valid = false;
            }
            supplied &= arg.has_value;
            slots.push(slot);
        }
        let required = parameters
            .iter()
            .enumerate()
            .all(|(index, param)| param.optional || used.contains(&index));
        Binding {
            slots,
            valid,
            fits: valid && (!self.complete || supplied && required),
            used,
        }
    }
}

// Opening is a known invocation '('. The slice starts in SQL code, so quote
// state never has to be guessed at an arbitrary window boundary.
pub(super) fn complete_arguments(
    text: &str,
    opening: usize,
    dialect: &str,
) -> Option<RoutineArguments> {
    let mut end = (opening + SIGNATURE_HELP_MAX_SCAN_BYTES).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let source = text.get(opening + 1..end)?;
    let masked = SqlParser::mask_sql_noise_for_dialect(source, dialect);
    let content = SqlParser::mask_sql_operand_context_for_dialect(source, dialect);
    scan_complete(text, &masked, &content, opening + 1, dialect)
}

pub(super) fn complete_arguments_in_masks(
    text: &str,
    masked: &str,
    content: &str,
    opening: usize,
    dialect: &str,
) -> Option<RoutineArguments> {
    scan_complete(
        text,
        masked.get(opening + 1..)?,
        content.get(opening + 1..)?,
        opening + 1,
        dialect,
    )
}

fn scan_complete(
    text: &str,
    masked: &str,
    content: &str,
    base: usize,
    dialect: &str,
) -> Option<RoutineArguments> {
    let mut nesting = 0;
    let mut start = 0;
    let mut ranges = Vec::new();
    for (at, ch) in masked.char_indices() {
        if at >= SIGNATURE_HELP_MAX_SCAN_BYTES - 1 {
            return None;
        }
        match ch {
            ')' if nesting == 0 => {
                if !content[start..at].trim().is_empty() {
                    ranges.push(base + start..base + at);
                } else if !ranges.is_empty() {
                    return None;
                }
                return (ranges.len() <= 256)
                    .then(|| RoutineArguments::from_ranges(text, &ranges, true, dialect));
            }
            ',' if nesting == 0 => {
                if content[start..at].trim().is_empty() || ranges.len() >= 256 {
                    return None;
                }
                ranges.push(base + start..base + at);
                start = at + 1;
            }
            ';' if nesting == 0 => return None,
            '(' | '[' | '{' => nesting += 1,
            ')' | ']' | '}' if nesting > 0 => nesting -= 1,
            _ => {}
        }
    }
    None
}
