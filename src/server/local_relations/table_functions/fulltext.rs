//! Fixed SQL Server full-text rowsets. KEY follows the full-text unique index,
//! which is not necessarily the primary key and is absent from current metadata.
use super::super::{local_relation_column, syntax::*};
use crate::schema::Column;

pub(super) fn columns(
    source: &str,
    upper: &str,
    parens: (usize, usize),
    alias_end: usize,
    uri: &str,
    line: u32,
) -> Option<Vec<Column>> {
    let (open, close) = parens;
    let args = split_top_level_ranges_in_searchable(upper, open + 1, close);
    if !(3..=5).contains(&args.len())
        || args
            .iter()
            .any(|(start, end)| source[*start..*end].trim().is_empty())
    {
        return None;
    }
    let (_, _, table_end) = read_local_identifier_path_after(source, args[0].0)?;
    if skip_local_whitespace(upper, table_end) != args[0].1 {
        return None;
    }
    if !valid_columns(source, upper, args[1]) {
        return None;
    }
    if args.len() == 5 {
        let language = skip_sql_trivia(source, args[3].0);
        if !upper[language..args[3].1].starts_with("LANGUAGE")
            || !local_keyword_boundary(upper, language, "LANGUAGE")
            || skip_sql_trivia(source, language + 8) >= args[3].1
        {
            return None;
        }
    }
    let mut columns: Vec<_> = [("KEY", "unknown"), ("RANK", "int")]
        .into_iter()
        .map(|(name, ty)| local_relation_column(name.into(), ty, uri, line))
        .collect();
    let alias_open = skip_local_whitespace(upper, alias_end);
    if upper.as_bytes().get(alias_open) == Some(&b'(') {
        let alias_close = matching_parenthesis_in_searchable(upper, alias_open)?;
        let names = split_top_level_ranges_in_searchable(upper, alias_open + 1, alias_close);
        if names.len() != 2 {
            return None;
        }
        for (column, (start, end)) in columns.iter_mut().zip(names) {
            let (name, _, next) = read_local_identifier_after(source, start)?;
            if skip_local_whitespace(upper, next) != end {
                return None;
            }
            column.name = name;
        }
    }
    Some(columns)
}

fn valid_columns(source: &str, upper: &str, range: (usize, usize)) -> bool {
    let (start, end) = range;
    let start = skip_sql_trivia(source, start);
    if upper[start..end].trim() == "*" {
        return true;
    }
    let ranges = if upper.as_bytes().get(start) == Some(&b'(') {
        let Some(close) = matching_parenthesis_in_searchable(upper, start) else {
            return false;
        };
        if close >= end || skip_local_whitespace(upper, close + 1) != end {
            return false;
        }
        split_top_level_ranges_in_searchable(upper, start + 1, close)
    } else {
        vec![(start, end)]
    };
    ranges.len() <= 256
        && ranges.into_iter().all(|(start, end)| {
            read_local_identifier_after(source, skip_sql_trivia(source, start))
                .is_some_and(|(_, _, next)| skip_local_whitespace(upper, next) == end)
        })
}
