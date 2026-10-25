//! Bounded local DECLARE and module-parameter metadata for one T-SQL batch. Reuse request
//! masks; never infer a variable from an arbitrary @reference or string body.
use super::{execute_context::keyword_at, routine_identity::read_parameter_name};
use std::collections::HashMap;

const MAX_BATCH_BYTES: usize = 512 * 1024;
const MAX_VARIABLES: usize = 4096;
pub(super) struct Variable {
    pub(super) name: String,
    pub(super) data_type: String,
    pub(super) scalar: bool,
    pub(super) name_start: usize,
    pub(super) visible_from: usize,
}
#[derive(Default)]
pub(super) struct Collected {
    pub(super) variables: Vec<Variable>,
    pub(super) is_incomplete: bool,
    pub(super) scope_start: usize,
}
fn next(content: &str, at: usize) -> usize {
    at + content[at..]
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(content.len() - at)
}
fn go(line: &str) -> bool {
    let mut words = line.split_whitespace();
    words
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("GO"))
        && words
            .next()
            .is_none_or(|word| word.parse::<u32>().is_ok_and(|n| n > 0))
        && words.next().is_none()
}
pub(super) fn batch_range(noise: &str, cursor: usize) -> std::ops::Range<usize> {
    let start = batch_start(noise, cursor);
    let mut at = 0;
    for line in noise.split_inclusive('\n') {
        if at >= cursor && go(line) {
            return start..at;
        }
        at += line.len();
    }
    start..noise.len()
}
fn batch_start(noise: &str, cursor: usize) -> usize {
    let (mut at, mut start) = (0, 0);
    for line in noise[..cursor].split_inclusive('\n') {
        if go(line) {
            start = at + line.len();
        }
        at += line.len();
    }
    start
}
fn declaration_end(noise: &str, start: usize, cursor: usize) -> (usize, usize, bool) {
    let (mut depth, mut type_end) = (0u32, None);
    for (at, ch) in noise[start..cursor].char_indices() {
        let at = start + at;
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '=' if depth == 0 && type_end.is_none() => type_end = Some(at),
            ',' | ';' if depth == 0 => return (type_end.unwrap_or(at), at, ch == ','),
            _ if depth == 0
                && [
                    "EXEC", "EXECUTE", "SELECT", "SET", "DECLARE", "GO", "BEGIN", "END", "PRINT",
                    "RETURN", "CREATE", "ALTER", "DROP", "INSERT", "UPDATE", "DELETE",
                ]
                .iter()
                .any(|word| keyword_at(noise, at, word)) =>
            {
                return (type_end.unwrap_or(at), at, false)
            }
            _ => {}
        }
    }
    (type_end.unwrap_or(cursor), cursor, false)
}
pub(super) fn declared(text: &str, noise: &str, content: &str, cursor: usize) -> Collected {
    let batch = batch_start(noise, cursor);
    if cursor - batch > MAX_BATCH_BYTES {
        return Collected {
            is_incomplete: true,
            scope_start: cursor,
            ..Default::default()
        };
    }
    let mut variables: HashMap<String, Option<(String, bool, usize, usize)>> = HashMap::new();
    let mut at = next(noise, batch);
    let module = super::tsql_module_scope::latest(noise, content, batch, cursor);
    if module.is_some_and(|start| start != next(content, batch)) {
        return Collected {
            scope_start: cursor,
            ..Default::default()
        };
    }
    if let Some(scope) = module
        .and_then(|start| super::tsql_module_parameters::scope(text, noise, content, start, cursor))
    {
        at = scope.body;
        for parameter in scope.parameters {
            variables
                .entry(parameter.name)
                .and_modify(|entry| *entry = None)
                .or_insert(Some((
                    parameter.data_type,
                    parameter.scalar,
                    parameter.name_start,
                    parameter.visible_from,
                )));
        }
    }
    let scope_start = at;
    while at < cursor && variables.len() < MAX_VARIABLES {
        if !matches!(noise.as_bytes()[at], b'D' | b'd') || !keyword_at(noise, at, "DECLARE") {
            at = next(noise, at + noise[at..].chars().next().unwrap().len_utf8());
            continue;
        }
        at += 7;
        loop {
            at = next(content, at);
            if at >= cursor || !text[at..].starts_with('@') || text[at..].starts_with("@@") {
                break;
            }
            let Some((name, length)) = read_parameter_name(&text[at..cursor], "sqlserver") else {
                break;
            };
            if name.len() > crate::tsql_identifier::MAX_NAME_UTF8_BYTES
                || !crate::tsql_identifier::is_complete_name(&name)
            {
                break;
            }
            let mut type_start = next(content, at + length);
            if keyword_at(noise, type_start, "AS") {
                type_start = next(content, type_start + 2);
            }
            if type_start >= cursor {
                break;
            }
            let (type_end, end, more) = declaration_end(noise, type_start, cursor);
            if !content[type_start..type_end].trim().is_empty() {
                let ty = text[type_start..type_end].trim();
                let ty = if ty.len() <= 256 { ty } else { "unknown" };
                let scalar = !keyword_at(noise, type_start, "TABLE")
                    && !keyword_at(noise, type_start, "CURSOR");
                variables
                    .entry(name)
                    .and_modify(|entry| *entry = None)
                    .or_insert_with(|| Some((ty.into(), scalar, at, type_end)));
            }
            at = end;
            if !more || variables.len() >= MAX_VARIABLES {
                break;
            }
            at += 1;
        }
    }
    let is_incomplete = at < cursor && variables.len() >= MAX_VARIABLES;
    let variables = variables
        .into_iter()
        .filter_map(|(name, ty)| {
            ty.map(|(data_type, scalar, name_start, visible_from)| Variable {
                name,
                data_type,
                scalar,
                name_start,
                visible_from,
            })
        })
        .collect();
    Collected {
        variables,
        is_incomplete,
        scope_start,
    }
}
