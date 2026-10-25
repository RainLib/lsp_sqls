//! CALL target selection owns its namespace and a complete leaf edit. It does
//! not run the general table/column/projection pipeline for a procedure name.
use super::completion_context::{position_to_byte_offset, CompletionCandidates};
use super::document_structure::position_for_offset;
use super::limits::{BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS, SIGNATURE_HELP_MAX_SCAN_BYTES};
use super::routine_identity::RoutineIdentity;
use crate::{
    parser::SqlParser,
    schema::{Schema, SchemaManager},
    sql_lexical::lexical_rules_for,
};
use std::collections::HashSet;
use tower_lsp::lsp_types::*;

struct Part {
    start: usize,
    end: usize,
    quote: Option<char>,
    closed: bool,
}
struct Target {
    qualifier: Vec<String>,
    prefix: String,
    quoted: bool,
    start: usize,
    end: usize,
    existing_call: bool,
}

fn next_code(content: &str, start: usize) -> usize {
    content[start..]
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map_or(content.len(), |(at, _)| start + at)
}

fn part(text: &str, start: usize, dialect: &str) -> Option<Part> {
    let first = text.get(start..)?.chars().next()?;
    let rules = lexical_rules_for(dialect);
    let quote = match first {
        '"' if !rules.double_quoted_strings => Some('"'),
        '`' => Some('`'),
        '[' if rules.bracket_identifiers => Some(']'),
        _ => None,
    };
    if let Some(close) = quote {
        let mut chars = text[start + 1..].char_indices().peekable();
        while let Some((at, ch)) = chars.next() {
            if ch == close {
                if chars.peek().is_some_and(|(_, next)| *next == close) {
                    chars.next();
                } else {
                    return Some(Part {
                        start,
                        end: start + 1 + at + ch.len_utf8(),
                        quote,
                        closed: true,
                    });
                }
            } else if ch == '\\' && rules.backslash_identifiers {
                chars.next();
            }
        }
        return Some(Part {
            start,
            end: text.len(),
            quote,
            closed: false,
        });
    }
    if !SqlParser::is_identifier_char(first) {
        return None;
    }
    let end = text[start..]
        .char_indices()
        .take_while(|(_, ch)| SqlParser::is_identifier_char(*ch))
        .map(|(at, ch)| start + at + ch.len_utf8())
        .last()?;
    Some(Part {
        start,
        end,
        quote: None,
        closed: true,
    })
}

fn decode_prefix(text: &str, part: &Part, cursor: usize) -> String {
    let end = cursor.min(part.end);
    let start = part.start.min(end);
    let raw = &text[start..end];
    if let Some(close) = part.quote {
        let raw = raw.get(1..).unwrap_or("");
        let raw = if part.closed && cursor >= part.end {
            raw.strip_suffix(close).unwrap_or(raw)
        } else {
            raw
        };
        raw.replace(&format!("{close}{close}"), &close.to_string())
    } else {
        raw.to_string()
    }
}

// None is an expression/other statement. Some(None) is an unresolvable CALL
// target: suppress unrelated suggestions without inventing a namespace.
fn target(text: &str, position: Position, dialect: &str) -> Option<Option<Target>> {
    let offset = position_to_byte_offset(text, position).min(text.len());
    if !text.as_bytes()[..offset].windows(4).any(|word| {
        word.eq_ignore_ascii_case(b"CALL")
            || super::execute_context::enabled(dialect) && word.eq_ignore_ascii_case(b"EXEC")
    }) {
        return None;
    }
    let execute = super::execute_context::enabled(dialect);
    let mut start = if execute {
        0
    } else {
        offset.saturating_sub(SIGNATURE_HELP_MAX_SCAN_BYTES)
    };
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let mut end = offset
        .saturating_add(SIGNATURE_HELP_MAX_SCAN_BYTES)
        .min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let source = &text[start..end];
    let cursor = offset - start;
    let noise = SqlParser::mask_sql_noise_for_dialect(source, dialect);
    let content = SqlParser::mask_sql_operand_context_for_dialect(source, dialect);
    let statement = if execute {
        super::execute_context::statement_start(&noise, cursor)
    } else {
        noise[..cursor].rfind(';').map_or(0, |at| at + 1)
    };
    let keyword = next_code(&noise, statement);
    let execute_start =
        super::execute_context::target_start(source, &noise, &content, statement, dialect);
    let execute = execute_start.is_some();
    if execute_start.is_none()
        && (keyword > cursor
            || !source
                .get(keyword..keyword + 4)
                .is_some_and(|word| word.eq_ignore_ascii_case("CALL"))
            || source
                .get(keyword + 4..)
                .and_then(|tail| tail.chars().next())
                .is_some_and(SqlParser::is_identifier_char))
    {
        return None;
    }
    if keyword + 4 > cursor {
        return None;
    }
    let mut at = execute_start.unwrap_or_else(|| next_code(&content, keyword + 4));
    let mut qualifier = Vec::new();
    loop {
        let Some(mut leaf) = part(source, at, dialect) else {
            if at >= cursor && content[cursor..at].trim().is_empty() {
                return Some(Some(Target {
                    qualifier,
                    prefix: String::new(),
                    quoted: false,
                    start: offset,
                    end: offset,
                    existing_call: execute
                        || source.get(at..).is_some_and(|tail| tail.starts_with('(')),
                }));
            }
            return Some(None);
        };
        if !leaf.closed {
            let tail = &source[cursor..];
            let boundary = tail.find(['\r', '\n', ';']).unwrap_or(tail.len());
            leaf.end = cursor + boundary;
        }
        let after = next_code(&content, leaf.end);
        if source
            .get(after..)
            .is_some_and(|tail| tail.starts_with('.'))
        {
            if cursor <= leaf.end || qualifier.len() >= 2 {
                return Some(None);
            }
            qualifier.push(source[leaf.start..leaf.end].into());
            at = next_code(&content, after + 1);
            continue;
        }
        if cursor > leaf.end
            && (!content[leaf.end..cursor].trim().is_empty()
                || source
                    .get(after..)
                    .is_some_and(|tail| tail.starts_with('('))
                    && after < cursor)
        {
            return None;
        }
        if cursor < leaf.start && !content[cursor..leaf.start].trim().is_empty() {
            return Some(None);
        }
        let prefix = decode_prefix(source, &leaf, cursor);
        return Some(Some(Target {
            qualifier,
            prefix,
            quoted: leaf.quote.is_some(),
            start: start + leaf.start.min(cursor),
            end: start + leaf.end.max(cursor),
            existing_call: execute
                || source
                    .get(after..)
                    .is_some_and(|tail| tail.starts_with('(')),
        }));
    }
}

fn quote_name(name: &str, dialect: &str) -> String {
    match lexical_rules_for(dialect).placeholder {
        crate::placeholder::SqlPlaceholderDialect::Mysql => {
            format!("`{}`", name.replace('`', "``"))
        }
        crate::placeholder::SqlPlaceholderDialect::SqlServer => {
            format!("[{}]", name.replace(']', "]]"))
        }
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

pub(super) fn completions(
    text: &str,
    position: Position,
    dialect: &str,
    current: impl FnOnce() -> Option<Schema>,
    manager: &SchemaManager,
) -> Option<CompletionCandidates> {
    let context = target(text, position, dialect)?;
    let Some(context) = context else {
        return Some(CompletionCandidates::default());
    };
    let mut path = context.qualifier.clone();
    path.push("__oxide_call_target__".into());
    let Some(identity) = RoutineIdentity::parse(&path.join("."), dialect) else {
        return Some(CompletionCandidates::default());
    };
    let current = current();
    let Some(schema) = identity.schema(current.as_ref(), manager, dialect) else {
        return Some(CompletionCandidates::default());
    };
    let strict_quoted = context.quoted
        && !matches!(
            lexical_rules_for(dialect).placeholder,
            crate::placeholder::SqlPlaceholderDialect::Mysql
                | crate::placeholder::SqlPlaceholderDialect::SqlServer
        );
    let mut matches = schema
        .functions
        .iter()
        .filter(|function| {
            function
                .routine_type
                .as_deref()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("procedure"))
                && !function.name.is_empty()
                && !function.name.contains('\0')
        })
        .filter_map(|function| {
            let rank = if strict_quoted {
                function.name.starts_with(&context.prefix).then_some(0)
            } else {
                crate::dialects::common::identifier_match_rank(&function.name, &context.prefix)
            }?;
            Some((rank, function))
        })
        .collect::<Vec<_>>();
    matches.sort_by_cached_key(|(rank, function)| (*rank, function.name.to_lowercase()));
    let range = Range {
        start: position_for_offset(text, context.start),
        end: position_for_offset(text, context.end),
    };
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    let mut is_incomplete = false;
    for (rank, function) in matches {
        let key =
            serde_json::to_string(&(&function.name, &function.parameters, &function.return_type))
                .expect("routine metadata serializes");
        if !seen.insert(key) {
            continue;
        }
        if items.len() >= BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS {
            is_incomplete = true;
            break;
        }
        let mut item = crate::dialects::common::create_function_item(function, &schema, false);
        let name = quote_name(&function.name, dialect);
        let new_text = if context.existing_call {
            name
        } else {
            format!("{name}($0)")
        };
        item.insert_text = Some(new_text.clone());
        item.insert_text_format = Some(if context.existing_call {
            InsertTextFormat::PLAIN_TEXT
        } else {
            InsertTextFormat::SNIPPET
        });
        item.text_edit = Some(CompletionTextEdit::Edit(TextEdit { range, new_text }));
        item.sort_text = Some(format!(
            "0:procedure:{rank}:{}",
            function.name.to_lowercase()
        ));
        items.push(item);
    }
    Some(CompletionCandidates {
        constraints: None,
        value_type: None,
        value_role: None,
        logical_continuation: None,
        pattern_escape: None,
        items,
        is_incomplete,
    })
}
