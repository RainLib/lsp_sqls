//! Explicit SQL table-function row shapes. This never evaluates search or JSON
//! or guesses user-defined function return types. Work shares the caller's
//! noise mask and is bounded by its statement budget plus depth/column caps.
use super::{local_relation_column, local_relation_table, scope_contains, syntax::*};
use crate::{
    parser::SqlParser,
    schema::{Column, Table},
};

mod fulltext;

const MAX_DEPTH: usize = 16;
const MAX_COLUMNS: usize = 256;

pub(super) fn declared_function_tables(
    source: &str,
    upper: &str,
    uri: &str,
    cursor: usize,
    dialect: Option<&str>,
) -> Vec<Table> {
    let position = crate::position::byte_position_at_end(&source[..cursor]);
    let qualifier = SqlParser::column_qualifier_before_position(source, position);
    let mut tables = Vec::new();
    for start in local_relation_source_starts(upper) {
        if !scope_contains(upper, start, cursor) {
            continue;
        }
        let mut start = skip_local_whitespace(upper, start);
        if let Some(next) = keyword(upper, start, upper.len(), "LATERAL") {
            start = next;
        }
        let Some((reference, reference_start, reference_end)) =
            read_local_identifier_path_after(source, start)
        else {
            continue;
        };
        let function = SqlParser::identifier_last_part(&reference).to_ascii_lowercase();
        let search_table =
            crate::search_functions::is_table_function(dialect.unwrap_or(""), &reference)
                && reference
                    .bytes()
                    .all(|byte| byte.is_ascii_alphabetic() || byte == b'_');
        if !search_table
            && !matches!(
                function.as_str(),
                "json_table"
                    | "openjson"
                    | "json_to_record"
                    | "jsonb_to_record"
                    | "json_to_recordset"
                    | "jsonb_to_recordset"
            )
        {
            continue;
        }
        let open = skip_local_whitespace(upper, reference_end);
        let Some(close) = matching_parenthesis_in_searchable(upper, open) else {
            continue;
        };
        let mut after = skip_sql_trivia(source, close + 1);
        let mut declarations = None;
        if function == "openjson" {
            if let Some(next) = keyword(upper, after, upper.len(), "WITH") {
                let Some(end) = matching_parenthesis_in_searchable(upper, next) else {
                    continue;
                };
                declarations = Some((next, end));
                after = skip_sql_trivia(source, end + 1);
            }
        }
        if keyword(upper, after, upper.len(), "AS").is_some() {
            after = skip_sql_trivia(source, after + 2);
        }
        let alias_info = read_local_identifier_after(source, after)
            .filter(|(alias, _, _)| !crate::token::Keywords::is_keyword(alias));
        let (alias, alias_start, alias_end) = match alias_info {
            Some(info) => info,
            None if search_table => (
                SqlParser::identifier_last_part(&reference),
                reference_start,
                close + 1,
            ),
            None => continue,
        };
        if let Some(qualifier) = qualifier.as_deref() {
            let quoted = matches!(source[alias_start..].chars().next(), Some('"' | '`' | '['));
            if qualifier != alias && (quoted || !qualifier.eq_ignore_ascii_case(&alias)) {
                continue;
            }
        }
        let line = source[..reference_start].matches('\n').count() as u32 + 1;
        let columns = if search_table {
            fulltext::columns(source, upper, (open, close), alias_end, uri, line)
        } else if function == "json_table" {
            // COLUMNS in the first JSON expression can be an ordinary field
            // name (u.columns). The declaration belongs to the second argument.
            let arguments = split_top_level_ranges_in_searchable(upper, open + 1, close);
            arguments
                .get(1)
                .and_then(|(start, end)| top_level_keyword(upper, *start, *end, "COLUMNS"))
                .and_then(|at| {
                    let opening = skip_local_whitespace(upper, at + 7);
                    let closing = matching_parenthesis_in_searchable(upper, opening)?;
                    if closing > close {
                        return None;
                    }
                    parse_columns(source, upper, opening, closing, true, uri)
                })
        } else if function == "openjson" {
            match declarations {
                Some((open, close)) => parse_columns(source, upper, open, close, false, uri),
                None => Some(
                    [
                        ("key", "nvarchar(4000)"),
                        ("value", "nvarchar(max)"),
                        ("type", "int"),
                    ]
                    .into_iter()
                    .map(|(name, ty)| local_relation_column(name.into(), ty, uri, line))
                    .collect(),
                ),
            }
        } else {
            let opening = skip_local_whitespace(upper, alias_end);
            matching_parenthesis_in_searchable(upper, opening)
                .and_then(|closing| parse_columns(source, upper, opening, closing, false, uri))
        };
        let Some(columns) = columns.filter(|columns| !columns.is_empty()) else {
            continue;
        };
        // Both spellings serve existing consumers: the parser normally resolves
        // aliases to a function reference; incomplete vendor grammar can retain
        // the alias itself. Only the requested alias contributes a qualified
        // shape, so two invocations of JSON_TABLE never overwrite each other.
        tables.push(local_relation_table(
            SqlParser::identifier_last_part(&reference),
            "TABLE FUNCTION",
            columns.clone(),
            uri,
            line,
        ));
        if alias != reference {
            tables.push(local_relation_table(
                alias,
                "TABLE FUNCTION",
                columns,
                uri,
                line,
            ));
        }
    }
    tables
}

fn keyword(upper: &str, start: usize, end: usize, word: &str) -> Option<usize> {
    let start = skip_local_whitespace(upper, start);
    (start + word.len() <= end
        && upper[start..].starts_with(word)
        && local_keyword_boundary(upper, start, word))
    .then(|| skip_local_whitespace(upper, start + word.len()))
}

fn top_level_keyword(upper: &str, start: usize, end: usize, word: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (relative, ch) in upper[start..end].char_indices() {
        let at = start + relative;
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if !ch.is_whitespace() && depth == 0 && keyword(upper, at, end, word).is_some() => {
                return Some(at)
            }
            _ => {}
        }
    }
    None
}

fn parse_columns(
    source: &str,
    upper: &str,
    open: usize,
    close: usize,
    nested: bool,
    uri: &str,
) -> Option<Vec<Column>> {
    let mut columns = Vec::new();
    collect_columns(source, upper, open, close, nested, uri, 0, &mut columns)?;
    Some(columns)
}

#[allow(clippy::too_many_arguments)]
fn collect_columns(
    source: &str,
    upper: &str,
    open: usize,
    close: usize,
    nested: bool,
    uri: &str,
    depth: usize,
    columns: &mut Vec<Column>,
) -> Option<()> {
    if depth >= MAX_DEPTH || close <= open || !upper[open..].starts_with('(') {
        return None;
    }
    for (start, end) in split_top_level_ranges_in_searchable(upper, open + 1, close) {
        // The keyword mask deliberately hides quoted identifiers. Skip only
        // trivia in the original text here, retaining quoted declaration names.
        let start = skip_sql_trivia(source, start);
        if nested && keyword(upper, start, end, "NESTED").is_some() {
            let at = top_level_keyword(upper, start, end, "COLUMNS")?;
            let open = skip_local_whitespace(upper, at + 7);
            let close = matching_parenthesis_in_searchable(upper, open)?;
            if close >= end {
                return None;
            }
            collect_columns(source, upper, open, close, nested, uri, depth + 1, columns)?;
            continue;
        }
        let (name, _, name_end) = read_local_identifier_after(source, start)?;
        let tail = skip_sql_trivia(source, name_end);
        let ty = if keyword(upper, tail, end, "FOR")
            .and_then(|at| keyword(upper, at, end, "ORDINALITY"))
            .is_some()
        {
            // The generated ordinal's exact SQL type differs between engines.
            // Keep the shape without inventing a product-specific numeric type.
            "unknown".to_owned()
        } else {
            declared_type(source, upper, tail, end)?
        };
        if columns.len() >= MAX_COLUMNS {
            return None;
        }
        columns.push(local_relation_column(
            name,
            &ty,
            uri,
            source[..start].matches('\n').count() as u32 + 1,
        ));
    }
    Some(())
}

fn declared_type(source: &str, upper: &str, start: usize, end: usize) -> Option<String> {
    let (_, _, mut finish) = read_local_identifier_path_after(source, start)?;
    if finish > end {
        return None;
    }
    let first = upper[start..finish].trim();
    // These are declaration modifiers, not types. Ordinary table hints and
    // untyped correlation lists cannot turn into fabricated typed columns.
    if matches!(
        first,
        "PATH" | "EXISTS" | "FOR" | "AS" | "DEFAULT" | "NULL" | "ERROR"
    ) {
        return None;
    }
    if let Some(next) = match first {
        "DOUBLE" => keyword(upper, finish, end, "PRECISION"),
        "CHARACTER" | "CHAR" | "BIT" => keyword(upper, finish, end, "VARYING"),
        _ => None,
    } {
        finish = next;
    }
    let precision = skip_local_whitespace(upper, finish);
    if precision < end && upper[precision..].starts_with('(') {
        let closing = matching_parenthesis_in_searchable(upper, precision)?;
        if closing >= end {
            return None;
        }
        finish = closing + 1;
    }
    if matches!(first, "TIMESTAMP" | "TIME") {
        for direction in ["WITH", "WITHOUT"] {
            if let Some(next) = keyword(upper, finish, end, direction)
                .and_then(|at| keyword(upper, at, end, "TIME"))
                .and_then(|at| keyword(upper, at, end, "ZONE"))
            {
                finish = next;
                break;
            }
        }
    }
    Some(source[start..finish].trim().to_owned())
}
