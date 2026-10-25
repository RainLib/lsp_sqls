//! Recursive CTEs bind the anchor shape before inspecting the recursive member.
//! No fixed-point evaluation or repeated type widening is performed. Anchor and
//! recursive inference share the ordinary per-output branch/depth budgets.
use super::{
    install_local_table, local_relation_column, local_relation_source_starts, local_relation_table,
    query_output::{Context, MAX_QUERY_DEPTH, MAX_SET_BRANCHES},
    read_local_identifier_after, skip_local_whitespace, skip_sql_trivia,
};
use crate::schema::{Column, Schema};
use std::collections::HashSet;

pub(super) struct Definition<'a> {
    pub name: &'a str,
    pub quoted: bool,
    pub names: &'a [String],
    pub body: &'a str,
    pub upper: &'a str,
    pub line: u32,
}

pub(super) struct Shape {
    pub columns: Vec<Column>,
    pub member_start: usize,
}

pub(super) fn implicit(dialect: Option<&str>) -> bool {
    matches!(dialect, Some("sqlserver" | "mssql" | "tsql" | "oracle"))
}

pub(super) fn infer(
    def: &Definition<'_>,
    schema: &Schema,
    uri: &str,
    dialect: Option<&str>,
    environment: super::query_output::Environment<'_>,
) -> Option<Shape> {
    let mut budget = MAX_SET_BRANCHES;
    infer_with_budget(def, schema, uri, dialect, 0, &mut budget, environment)
}

pub(super) fn infer_with_budget(
    def: &Definition<'_>,
    schema: &Schema,
    uri: &str,
    dialect: Option<&str>,
    depth: usize,
    budget: &mut usize,
    environment: super::query_output::Environment<'_>,
) -> Option<Shape> {
    let (start, end, wrappers) = unwrapped(def.body, def.upper)?;
    let depth = depth.saturating_add(wrappers);
    let inner = Definition {
        name: def.name,
        quoted: def.quoted,
        names: def.names,
        body: &def.body[start..end],
        upper: &def.upper[start..end],
        line: def.line,
    };
    infer_body(&inner, schema, uri, dialect, depth, budget, environment).map(|mut shape| {
        shape.member_start += start;
        shape
    })
}

fn infer_body(
    def: &Definition<'_>,
    schema: &Schema,
    uri: &str,
    dialect: Option<&str>,
    depth: usize,
    budget: &mut usize,
    environment: super::query_output::Environment<'_>,
) -> Option<Shape> {
    let unions = unions(def.body, def.upper);
    let mut starts = vec![0];
    starts.extend(unions.iter().map(|(_, after)| *after));
    let mut first = None;
    for (index, start) in starts.iter().enumerate() {
        let end = unions.get(index).map_or(def.body.len(), |(at, _)| *at);
        if references_self(
            &def.body[*start..end],
            &def.upper[*start..end],
            def,
            dialect,
        ) {
            first = Some(index);
            break;
        }
    }
    let first = first?;
    let unknown = || Shape {
        columns: def
            .names
            .iter()
            .map(|name| local_relation_column(name.clone(), "unknown", uri, def.line))
            .collect(),
        member_start: starts[first],
    };
    if first == 0 || starts.len() > MAX_SET_BRANCHES {
        return Some(unknown());
    }
    let anchor_end = unions[first - 1].0;
    let member_start = starts[first];
    let context = Context {
        schema,
        uri,
        line: def.line,
        dialect,
        environment,
    };
    let anchor = context.query(
        &def.body[..anchor_end],
        &def.upper[..anchor_end],
        depth + 1,
        budget,
    );
    let width = anchor.columns.len();
    let arity = anchor.known_arity
        && width > 0
        && def.names.len() <= width
        && (dialect == Some("postgres") || def.names.is_empty() || def.names.len() == width);
    let mut columns = context.finish(anchor, def.names);
    if !arity {
        for column in &mut columns {
            column.data_type = "unknown".into();
            column.nullable = true;
        }
        return Some(Shape {
            columns,
            member_start,
        });
    }
    for column in &mut columns {
        column.nullable = true;
    }
    let mut working = schema.clone();
    install_local_table(
        &mut working,
        local_relation_table(
            def.name.into(),
            "COMMON TABLE EXPRESSION",
            columns.clone(),
            uri,
            def.line,
        ),
    );
    let recursive = Context {
        schema: &working,
        uri,
        line: def.line,
        dialect,
        environment,
    }
    .query(
        &def.body[member_start..],
        &def.upper[member_start..],
        depth + 1,
        budget,
    );
    let compatible = recursive.known_arity && recursive.columns.len() == width;
    for (index, column) in columns.iter_mut().enumerate() {
        let supported = matches!(
            dialect,
            Some("postgres" | "mysql" | "sqlserver" | "tsql" | "mssql")
        );
        let mismatch = matches!(dialect, Some("postgres" | "sqlserver" | "tsql" | "mssql"))
            && recursive.columns.get(index).is_some_and(|other| {
                other.data_type != "unknown"
                    && other.data_type != "null"
                    && canonical(&column.data_type) != canonical(&other.data_type)
            });
        if !compatible || !supported || mismatch {
            column.data_type = "unknown".into();
        }
    }
    let mut seen = HashSet::new();
    columns
        .retain(|column| !column.name.is_empty() && seen.insert(column.name.to_ascii_lowercase()));
    Some(Shape {
        columns,
        member_start,
    })
}

fn canonical(ty: &str) -> String {
    if ty.contains('"') {
        return ty.into();
    }
    let lower = ty.to_ascii_lowercase();
    match lower.as_str() {
        "int" | "int4" => "integer".into(),
        "int8" => "bigint".into(),
        "int2" => "smallint".into(),
        "float4" => "real".into(),
        "float8" => "double precision".into(),
        _ => {
            let base = lower
                .split('(')
                .next()
                .unwrap_or("")
                .trim_end_matches("[]")
                .trim();
            if matches!(
                base,
                "smallint"
                    | "integer"
                    | "int"
                    | "bigint"
                    | "numeric"
                    | "decimal"
                    | "real"
                    | "double precision"
                    | "text"
                    | "varchar"
                    | "character varying"
                    | "char"
                    | "boolean"
                    | "bool"
                    | "date"
                    | "timestamp"
            ) {
                lower
            } else {
                ty.into()
            }
        }
    }
}

fn unwrapped(source: &str, upper: &str) -> Option<(usize, usize, usize)> {
    let mut start = skip_sql_trivia(source, 0);
    let mut end = source.trim_end().len();
    for depth in 0..MAX_QUERY_DEPTH {
        if upper.as_bytes().get(start) != Some(&b'(') {
            return Some((start, end, depth));
        }
        let close = super::matching_parenthesis_in_searchable(upper, start)?;
        if skip_sql_trivia(source, close + 1) < end {
            return Some((start, end, depth));
        }
        start = skip_sql_trivia(source, start + 1);
        end = close;
    }
    None
}

fn references_self(source: &str, upper: &str, def: &Definition<'_>, dialect: Option<&str>) -> bool {
    let Some((start, end, _)) = unwrapped(source, upper) else {
        return false;
    };
    let source = &source[start..end];
    let upper = &upper[start..end];
    // Nested WITH has its own bindings. Until that graph is shared, do not
    // treat a same-spelled inner CTE as this definition's recursive reference.
    if upper.starts_with("WITH") && super::local_keyword_boundary(upper, 0, "WITH") {
        return false;
    }
    let positions: HashSet<_> = local_relation_source_starts(upper).into_iter().collect();
    let mut depth = 0usize;
    for (index, byte) in upper.bytes().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth != 0 || !positions.contains(&index) {
            continue;
        }
        let Some((name, start, end)) =
            read_local_identifier_after(source, skip_sql_trivia(source, index))
        else {
            continue;
        };
        if upper.as_bytes().get(skip_local_whitespace(upper, end)) == Some(&b'.') {
            continue;
        }
        let quoted = matches!(source.as_bytes()[start], b'"' | b'`' | b'[');
        let matches = if dialect == Some("postgres") {
            let expected = if def.quoted {
                def.name.to_string()
            } else {
                def.name.to_ascii_lowercase()
            };
            let actual = if quoted {
                name.clone()
            } else {
                name.to_ascii_lowercase()
            };
            actual == expected
        } else {
            name.eq_ignore_ascii_case(def.name)
        };
        if matches {
            return true;
        }
    }
    false
}

// Top-level UNION boundaries preserve nested SELECTs/parentheses and operators
// inside strings, comments and quoted identifiers, which are already masked.
fn unions(source: &str, upper: &str) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    let mut depth = 0usize;
    let bytes = upper.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b'U' if depth == 0
                && bytes[index..].starts_with(b"UNION")
                && super::local_keyword_boundary(upper, index, "UNION") =>
            {
                let mut after = skip_sql_trivia(source, index + 5);
                for modifier in ["ALL", "DISTINCT"] {
                    if upper[after..].starts_with(modifier)
                        && super::local_keyword_boundary(upper, after, modifier)
                    {
                        after = skip_sql_trivia(source, after + modifier.len());
                        break;
                    }
                }
                result.push((index, after));
                index = after;
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    result
}

#[cfg(test)]
mod tests;
