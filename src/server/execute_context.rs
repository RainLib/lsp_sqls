//! T-SQL static EXEC calls have no argument parentheses. Keep their lexical
//! owner separate from ordinary invocation scanning and never inspect dynamic SQL.
use super::{
    completion_context::position_to_byte_offset, limits::SIGNATURE_HELP_MAX_SCAN_BYTES,
    routine_arguments::RoutineArguments, routine_context::RoutineCallContext,
    routine_identity::read_parameter_name,
};
use crate::{
    parser::SqlParser, placeholder::SqlPlaceholderDialect, sql_lexical::lexical_rules_for,
};
use tower_lsp::lsp_types::Position;

pub(super) fn enabled(dialect: &str) -> bool {
    lexical_rules_for(dialect).placeholder == SqlPlaceholderDialect::SqlServer
}
fn next(content: &str, at: usize) -> usize {
    at + content[at..]
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(content.len() - at)
}
pub(super) fn keyword_at(content: &str, at: usize, keyword: &str) -> bool {
    !content[..at]
        .chars()
        .next_back()
        .is_some_and(SqlParser::is_identifier_char)
        && content
            .get(at..at + keyword.len())
            .is_some_and(|word| word.eq_ignore_ascii_case(keyword))
        && content
            .get(at + keyword.len()..)
            .and_then(|tail| tail.chars().next())
            .is_none_or(|ch| !SqlParser::is_identifier_char(ch))
}
// T-SQL modules and IF/ELSE bodies commonly omit semicolons. A standalone
// top-level EXEC is a boundary even after BEGIN/IF on the same line. Masked
// quotes/comments and @references cannot introduce the keyword.
fn boundaries(noise: &str) -> impl Iterator<Item = usize> + '_ {
    let mut depth = 0u32;
    let candidates = std::iter::once(0).chain(noise.char_indices().filter_map(move |(at, ch)| {
        let call = at > 0
            && depth == 0
            && matches!(ch, 'E' | 'e')
            && (keyword_at(noise, at, "EXEC") || keyword_at(noise, at, "EXECUTE"));
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ';' if depth == 0 => {
                return Some(at + 1);
            }
            _ => {}
        }
        call.then_some(at)
    }));
    let mut previous = None;
    candidates.filter_map(move |at| {
        let start = next(noise, at);
        if previous == Some(start) {
            return None;
        }
        previous = Some(start);
        Some(start)
    })
}
pub(super) fn statement_start(noise: &str, cursor: usize) -> usize {
    boundaries(noise)
        .take_while(|start| *start <= cursor)
        .last()
        .unwrap_or(0)
}
// Caller supplies same-byte-length lexical masks. The statement must start at
// an actual delimiter (or byte zero), never an arbitrary truncated prefix.
pub(super) fn target_start(
    text: &str,
    noise: &str,
    content: &str,
    statement: usize,
    dialect: &str,
) -> Option<usize> {
    if !enabled(dialect) {
        return None;
    }
    let keyword = next(noise, statement);
    let length = if keyword_at(noise, keyword, "EXECUTE") {
        7
    } else if keyword_at(noise, keyword, "EXEC") {
        4
    } else {
        return None;
    };
    let mut at = next(content, keyword + length);
    if ["AS", "ON", "TO", "WITH"]
        .iter()
        .any(|word| keyword_at(noise, at, word))
    {
        return None;
    }
    if text.get(at..)?.starts_with('@') {
        let (_, length) = read_parameter_name(&text[at..], dialect)?;
        let after = next(content, at + length);
        if !content[after..].starts_with('=') || content[after..].starts_with("==") {
            return None;
        }
        at = next(content, after + 1);
    }
    if text.get(at..)?.starts_with(['@', '(', '\'']) {
        return None;
    }
    Some(at)
}

fn reference(text: &str, content: &str, mut at: usize, dialect: &str) -> Option<(String, usize)> {
    let mut parts = Vec::new();
    loop {
        let (name, length) = read_parameter_name(text.get(at..)?, dialect)?;
        if name.starts_with('@') {
            return None;
        }
        let end = at + length;
        parts.push(text[at..end].to_string());
        at = next(content, end);
        if !content[at..].starts_with('.') {
            return Some((parts.join("."), end));
        }
        if parts.len() >= 3 {
            return None;
        }
        at = next(content, at + 1);
    }
}

fn scan(
    text: &str,
    noise: &str,
    content: &str,
    statement: usize,
    cursor: usize,
    dialect: &str,
) -> Option<RoutineCallContext> {
    let target = target_start(text, noise, content, statement, dialect)?;
    let (name, end) = reference(text, content, target, dialect)?;
    if cursor <= end || cursor - statement > SIGNATURE_HELP_MAX_SCAN_BYTES {
        return None;
    }
    let limit = (end + SIGNATURE_HELP_MAX_SCAN_BYTES).min(text.len());
    let mut ranges = Vec::new();
    let mut start = end;
    let mut nesting = 0u32;
    let mut stop = text.len();
    let mut complete = false;
    for (relative, ch) in noise[end..].char_indices() {
        let at = end + relative;
        if at >= limit {
            return None;
        }
        match ch {
            '(' | '[' | '{' => nesting += 1,
            ')' | ']' | '}' => nesting = nesting.checked_sub(1)?,
            ',' if nesting == 0 => {
                if ranges.len() >= 255 || content[start..at].trim().is_empty() {
                    return None;
                }
                ranges.push(start..at);
                start = at + 1;
            }
            ';' if nesting == 0 => {
                stop = at;
                complete = true;
                break;
            }
            _ if nesting == 0
                && [
                    "WITH", "SELECT", "EXEC", "EXECUTE", "DECLARE", "SET", "INSERT", "UPDATE",
                    "DELETE", "MERGE", "GO", "RETURN", "PRINT", "BEGIN", "END", "ELSE", "IF",
                    "WHILE",
                ]
                .iter()
                .any(|word| keyword_at(noise, at, word)) =>
            {
                stop = at;
                complete = true;
                break;
            }
            _ => {}
        }
    }
    if cursor > stop {
        return None;
    }
    ranges.push(start..stop);
    let active = ranges
        .iter()
        .position(|range| range.start <= cursor && cursor <= range.end)?;
    let current = ranges[active].start..cursor;
    let has_content = !content[current.clone()].trim().is_empty();
    let mut supplied = RoutineArguments::from_ranges(text, &ranges, complete, dialect);
    if ranges.len() == 1 && content[ranges[0].clone()].trim().is_empty() {
        supplied.values.clear();
    }
    // All arguments after the caret remain available to name binding and
    // overload selection; the active range still ends at the actual caret.
    let mut arguments = ranges[..active].to_vec();
    arguments.push(current);
    Some(RoutineCallContext {
        name,
        active_group: 0,
        active_parameter: active as u32,
        current_argument_has_content: has_content,
        arguments,
        explicit_variadic: false,
        procedure_call: true,
        supplied,
    })
}

pub(super) fn call_at_position(
    text: &str,
    position: Position,
    dialect: &str,
) -> Option<RoutineCallContext> {
    context_at_position(text, position, dialect).map(|context| context.call)
}

pub(super) struct ExecuteContext {
    pub(super) call: RoutineCallContext,
    pub(super) noise: String,
    pub(super) content: String,
}

pub(super) fn context_at_position(
    text: &str,
    position: Position,
    dialect: &str,
) -> Option<ExecuteContext> {
    if !enabled(dialect) {
        return None;
    }
    let cursor = position_to_byte_offset(text, position).min(text.len());
    if !text.as_bytes()[..cursor]
        .windows(4)
        .any(|word| word.eq_ignore_ascii_case(b"EXEC"))
    {
        return None;
    }
    // Start from document byte zero so an opening quote/comment before the
    // bounded invocation window cannot manufacture an EXEC statement.
    let noise = SqlParser::mask_sql_noise_for_dialect(text, dialect);
    let content = SqlParser::mask_sql_operand_context_for_dialect(text, dialect);
    let statement = statement_start(&noise, cursor);
    let call = scan(text, &noise, &content, statement, cursor, dialect)?;
    Some(ExecuteContext {
        call,
        noise,
        content,
    })
}

pub(super) fn calls_in_masks<'a>(
    text: &'a str,
    noise: &'a str,
    content: &'a str,
    end: usize,
    dialect: &'a str,
) -> impl Iterator<Item = RoutineCallContext> + 'a {
    boundaries(noise)
        .take_while(move |statement| enabled(dialect) && *statement <= end)
        .filter_map(move |statement| {
            let target = target_start(text, noise, content, statement, dialect)?;
            let (_, module_end) = reference(text, content, target, dialect)?;
            let cursor = next(content, module_end);
            scan(text, noise, content, statement, cursor, dialect)
        })
        .take(500)
}
