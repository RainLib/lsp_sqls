//! Static EXEC actuals are constants/variables, not SQL expression completion
//! sites. This owner runs before generic schema projection and keyword work.
use super::{
    completion_context::{position_to_byte_offset, CompletionCandidates},
    document_structure::position_for_offset,
    execute_context,
    limits::BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS,
    routine_arguments::argument,
    routine_context::RoutineCallContext,
    routine_identity::{read_parameter_name, RoutineIdentity},
    routine_overloads::canonical_type,
    routine_parameters::RoutineParameters,
};
use crate::schema::{Schema, SchemaManager};
use tower_lsp::lsp_types::*;

#[derive(Default)]
struct Parameter {
    default: bool,
    output: bool,
    data_type: Option<String>,
}
fn parameter(
    call: &RoutineCallContext,
    current: Option<&Schema>,
    manager: &SchemaManager,
    dialect: &str,
) -> Parameter {
    let Some(identity) = RoutineIdentity::parse(&call.name, dialect) else {
        return Parameter::default();
    };
    let Some(schema) = identity.schema(current, manager, dialect) else {
        return Parameter::default();
    };
    let mut result = Parameter {
        default: true,
        output: true,
        data_type: None,
    };
    let (mut count, mut same_type) = (0, true);
    for function in schema.functions.iter().filter(|f| {
        identity.matches(f, dialect)
            && f.routine_type
                .as_deref()
                .is_some_and(|k| k.eq_ignore_ascii_case("procedure"))
    }) {
        let parameters = RoutineParameters::new(function, dialect, true);
        let bound = call.supplied.bind(&parameters, dialect);
        if !bound.valid {
            return Parameter::default();
        }
        let Some(p) = bound
            .slots
            .get(call.active_parameter as usize)
            .copied()
            .flatten()
            .and_then(|slot| parameters.parameter(slot))
        else {
            return Parameter::default();
        };
        result.default &= p.optional;
        result.output &= matches!(p.mode_name(), Some("OUT" | "INOUT"));
        let ty = canonical_type(&p.data_type, "sqlserver");
        if count == 0 {
            result.data_type = ty;
        } else {
            same_type &= result.data_type == ty;
        }
        count += 1;
    }
    if count == 0 {
        return Parameter::default();
    }
    if !same_type {
        result.data_type = None;
    }
    result
}
struct Site {
    start: usize,
    end: usize,
    prefix: String,
    output: bool,
}
fn site(text: &str, content: &str, range: std::ops::Range<usize>, cursor: usize) -> Option<Site> {
    if cursor < range.start || cursor > range.end {
        return None;
    }
    let start = range.start
        + content[range.start..cursor]
            .find(|ch: char| !ch.is_whitespace())
            .unwrap_or(cursor - range.start);
    if cursor < start || cursor > range.end {
        return None;
    }
    let raw = &text[start..range.end];
    let first = read_parameter_name(raw, "sqlserver");
    let end = first.as_ref().map_or(start, |(_, length)| start + length);
    if cursor <= end {
        let tail = content[end..range.end].trim();
        if !tail.is_empty()
            && !(raw.starts_with('@')
                && (tail.eq_ignore_ascii_case("OUTPUT") || tail.eq_ignore_ascii_case("OUT")))
        {
            return None;
        }
        return Some(Site {
            start,
            end,
            prefix: text[start..cursor].to_ascii_lowercase(),
            output: false,
        });
    }
    // Only an unquoted local-variable operand can carry OUTPUT. No numeric,
    // function expression, string literal or existing operator is rewritten.
    if !raw.starts_with('@') || raw.starts_with("@@") {
        return None;
    }
    let output_start = end
        + content[end..range.end]
            .find(|ch: char| !ch.is_whitespace())
            .unwrap_or(range.end - end);
    if cursor < output_start {
        return Some(Site {
            start: cursor,
            end: cursor,
            prefix: String::new(),
            output: true,
        });
    }
    let word = &text[output_start..range.end];
    let length = word
        .chars()
        .take_while(|ch| ch.is_ascii_alphabetic())
        .count();
    if cursor > output_start + length
        || !content[output_start + length..range.end].trim().is_empty()
    {
        return None;
    }
    Some(Site {
        start: output_start,
        end: output_start + length,
        prefix: text[output_start..cursor].to_ascii_lowercase(),
        output: true,
    })
}
fn item(
    text: &str,
    site: &Site,
    label: &str,
    kind: CompletionItemKind,
    detail: String,
    sort_text: String,
) -> CompletionItem {
    CompletionItem {
        label: label.into(),
        kind: Some(kind),
        detail: Some(detail),
        sort_text: Some(sort_text),
        insert_text: Some(label.into()),
        insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: Range {
                start: position_for_offset(text, site.start),
                end: position_for_offset(text, site.end),
            },
            new_text: label.into(),
        })),
        ..Default::default()
    }
}
pub(super) fn completions(
    text: &str,
    position: Position,
    dialect: &str,
    current: impl FnOnce() -> Option<Schema>,
    manager: &SchemaManager,
) -> Option<CompletionCandidates> {
    let mut context = execute_context::context_at_position(text, position, dialect)?;
    let active = context.call.active_parameter as usize;
    let cursor = position_to_byte_offset(text, position);
    if context.call.supplied.values.is_empty() {
        if let Some(range) = context.call.arguments.last() {
            context
                .call
                .supplied
                .values
                .push(argument(text, range.clone(), dialect));
        }
    }
    let current = current();
    let parameter = parameter(&context.call, current.as_ref(), manager, dialect);
    let mut values = Vec::new();
    let mut variables_incomplete = false;
    if let Some(argument) = context.call.supplied.values.get(active) {
        let named_before = context.call.supplied.values[..active]
            .iter()
            .any(|arg| arg.name.is_some());
        if argument.name.is_some() || !named_before {
            let mut value = argument.value.clone();
            // An empty value followed by a comment has its trimmed scanner
            // start after the caret. Insert at the caret without consuming the
            // comment; parameter-name positions before '=' remain name sites.
            if !argument.has_value
                && value.start > cursor
                && (argument.name.is_none()
                    || context.noise[argument.full.start..cursor].contains('='))
            {
                value.start = cursor;
            }
            if let Some(site) = site(text, &context.content, value, cursor) {
                if site.output {
                    if parameter.output && "output".starts_with(&site.prefix) {
                        values.push(item(
                            text,
                            &site,
                            "OUTPUT",
                            CompletionItemKind::KEYWORD,
                            "EXEC output parameter".into(),
                            "2:output".into(),
                        ));
                    }
                } else {
                    for label in ["NULL", "DEFAULT"] {
                        if label == "DEFAULT" && !parameter.default {
                            continue;
                        }
                        if label.to_ascii_lowercase().starts_with(&site.prefix) {
                            values.push(item(
                                text,
                                &site,
                                label,
                                CompletionItemKind::KEYWORD,
                                "EXEC argument literal".into(),
                                format!("2:{label}"),
                            ));
                        }
                    }
                    let variables = super::tsql_variables::declared(
                        text,
                        &context.noise,
                        &context.content,
                        cursor,
                    );
                    variables_incomplete = variables.is_incomplete;
                    for variable in variables.variables {
                        if !variable.scalar {
                            continue;
                        }
                        let lower = variable.name.to_ascii_lowercase();
                        if !lower.starts_with(&site.prefix) {
                            continue;
                        }
                        let matching = canonical_type(&variable.data_type, "sqlserver")
                            .is_some_and(|ty| Some(ty) == parameter.data_type);
                        values.push(item(
                            text,
                            &site,
                            &variable.name,
                            CompletionItemKind::VARIABLE,
                            format!("Variable: {} ({})", variable.name, variable.data_type),
                            format!(
                                "1:variable:{}:{}:{lower}",
                                u8::from(lower != site.prefix),
                                u8::from(!matching)
                            ),
                        ));
                    }
                }
            }
        }
    }
    let named = super::named_completion::completions_for_call(
        text,
        position,
        dialect,
        || current,
        manager,
        context.call,
    );
    let is_incomplete = named.is_incomplete
        || variables_incomplete
        || values.len() + named.items.len() > BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS;
    values.extend(named.items);
    values.sort_by(|a, b| a.sort_text.cmp(&b.sort_text).then(a.label.cmp(&b.label)));
    values.truncate(BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS);
    Some(CompletionCandidates {
        constraints: None,
        value_type: None,
        value_role: None,
        logical_continuation: None,
        pattern_escape: None,
        items: values,
        is_incomplete,
    })
}
