//! Projection slots remain positional until explicit relation names are applied.
//! Each set branch owns its input bindings; PostgreSQL rules are not reused for
//! other engines. Unknown arity invalidates positional type propagation.
use super::{
    derived_tables_with_inference,
    expression_types::{merge_set_types, ExpressionType},
    find_top_level_keyword_in_searchable, install_local_table, local_keyword_boundary,
    local_relation_column, matching_parenthesis_in_searchable, projection,
    read_local_identifier_after, skip_sql_trivia, split_top_level_ranges_in_searchable,
};
use crate::{
    parser::SqlParser,
    schema::{Column, Schema},
    token::Keywords,
};
use std::collections::HashSet;

pub(super) const MAX_SET_BRANCHES: usize = 64;
pub(super) const MAX_QUERY_DEPTH: usize = 32;

#[derive(Default)]
pub(super) struct Output {
    pub(super) columns: Vec<Column>,
    pub(super) known_arity: bool,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Environment<'a> {
    pub(super) origin: Option<super::provenance::QueryOrigin<'a>>,
    pub(super) routines: Option<&'a crate::schema::SchemaManager>,
    pub(super) variables: Option<super::variables::Origin<'a>>,
}

pub(super) struct Context<'a> {
    pub(super) schema: &'a Schema,
    pub(super) uri: &'a str,
    pub(super) line: u32,
    pub(super) dialect: Option<&'a str>,
    pub(super) environment: Environment<'a>,
}

pub(in crate::server) fn infer_query_output_columns(
    query: &str,
    schema: &Schema,
    uri: &str,
    source_line: u32,
    dialect: Option<&str>,
) -> Vec<Column> {
    infer_query_output_columns_with_names(query, schema, uri, source_line, dialect, &[])
}

pub(super) fn infer_query_output_columns_with_routines(
    query: &str,
    schema: &Schema,
    uri: &str,
    source_line: u32,
    dialect: Option<&str>,
    routines: Option<&crate::schema::SchemaManager>,
) -> Vec<Column> {
    if routines.is_none() {
        return infer_query_output_columns(query, schema, uri, source_line, dialect);
    }
    let parsed = super::provenance::ParsedOrigin::new(query, dialect);
    infer_with_environment(
        query,
        schema,
        uri,
        source_line,
        dialect,
        &[],
        Environment {
            origin: parsed.view(),
            routines,
            variables: None,
        },
    )
}

pub(super) fn infer_query_output_columns_with_names(
    query: &str,
    schema: &Schema,
    uri: &str,
    source_line: u32,
    dialect: Option<&str>,
    names: &[String],
) -> Vec<Column> {
    let parsed = super::provenance::ParsedOrigin::new(query, dialect);
    infer_with_environment(
        query,
        schema,
        uri,
        source_line,
        dialect,
        names,
        Environment {
            origin: parsed.view(),
            routines: None,
            variables: None,
        },
    )
}

pub(super) fn infer_with_environment(
    query: &str,
    schema: &Schema,
    uri: &str,
    source_line: u32,
    dialect: Option<&str>,
    names: &[String],
    environment: Environment<'_>,
) -> Vec<Column> {
    let upper = SqlParser::mask_sql_noise(query).to_ascii_uppercase();
    let context = Context {
        schema,
        uri,
        line: source_line,
        dialect,
        environment,
    };
    let mut budget = MAX_SET_BRANCHES;
    let output = context.query(query, &upper, 0, &mut budget);
    let mut columns = context.finish(output, names);
    let mut seen = HashSet::new();
    columns
        .retain(|column| !column.name.is_empty() && seen.insert(column.name.to_ascii_lowercase()));
    columns
}

impl Context<'_> {
    pub(super) fn finish(&self, mut output: Output, names: &[String]) -> Vec<Column> {
        if !output.known_arity && !names.is_empty() {
            return names
                .iter()
                .map(|name| local_relation_column(name.clone(), "unknown", self.uri, self.line))
                .collect();
        }
        for (column, name) in output.columns.iter_mut().zip(names) {
            column.name = name.clone();
        }
        for name in names.iter().skip(output.columns.len()) {
            output.columns.push(local_relation_column(
                name.clone(),
                "unknown",
                self.uri,
                self.line,
            ));
        }
        for column in &mut output.columns {
            if column.data_type == "null"
                || column.data_type == super::expression_types::UNTYPED_STRING
            {
                column.data_type = if self.dialect == Some("postgres") {
                    "text"
                } else {
                    "unknown"
                }
                .into();
            }
        }
        output.columns
    }

    pub(super) fn query(
        &self,
        query: &str,
        upper: &str,
        depth: usize,
        budget: &mut usize,
    ) -> Output {
        if depth >= MAX_QUERY_DEPTH || *budget == 0 {
            return Output::default();
        }
        let start = skip_sql_trivia(query, 0);
        let end = query.trim_end().trim_end_matches(';').trim_end().len();
        if start >= end {
            return Output::default();
        }
        if upper.as_bytes()[start] == b'(' {
            if let Some(close) = matching_parenthesis_in_searchable(upper, start) {
                if close + 1 == end || skip_sql_trivia(query, close + 1) >= end {
                    return self.query(
                        &query[start + 1..close],
                        &upper[start + 1..close],
                        depth + 1,
                        budget,
                    );
                }
            }
        }
        if upper[start..].starts_with("WITH") && local_keyword_boundary(upper, start, "WITH") {
            return super::with_queries::infer(self, query, upper, start, depth, budget);
        }
        if let Some((offset, length)) = set_split(&upper[start..end]) {
            let offset = start + offset;
            let mut right = skip_sql_trivia(query, offset + length);
            for modifier in ["ALL", "DISTINCT"] {
                if upper[right..].starts_with(modifier)
                    && local_keyword_boundary(upper, right, modifier)
                {
                    right = skip_sql_trivia(query, right + modifier.len());
                    break;
                }
            }
            let left = self.query(
                &query[start..offset],
                &upper[start..offset],
                depth + 1,
                budget,
            );
            let right = self.query(&query[right..end], &upper[right..end], depth + 1, budget);
            return self.merge(left, right);
        }
        *budget -= 1;
        self.select(&query[start..end], &upper[start..end], depth, budget)
    }

    fn merge(&self, mut left: Output, right: Output) -> Output {
        let compatible =
            left.known_arity && right.known_arity && left.columns.len() == right.columns.len();
        for (index, column) in left.columns.iter_mut().enumerate() {
            let merged = compatible
                .then(|| right.columns.get(index))
                .flatten()
                .and_then(|other| {
                    merge_set_types(
                        self.dialect,
                        ExpressionType {
                            data_type: column.data_type.clone(),
                            nullable: column.nullable,
                        },
                        ExpressionType {
                            data_type: other.data_type.clone(),
                            nullable: other.nullable,
                        },
                    )
                });
            column.data_type = merged
                .as_ref()
                .map_or_else(|| "unknown".into(), |ty| ty.data_type.clone());
            column.nullable = merged.is_none_or(|ty| ty.nullable);
        }
        left.known_arity = compatible;
        left
    }

    fn select(&self, query: &str, upper: &str, depth: usize, budget: &mut usize) -> Output {
        let Some(select) = find_top_level_keyword_in_searchable(upper, 0, "SELECT") else {
            return Output::default();
        };
        let mut start = skip_sql_trivia(query, select + "SELECT".len());
        for modifier in ["DISTINCT", "ALL"] {
            if upper[start..].starts_with(modifier)
                && local_keyword_boundary(upper, start, modifier)
            {
                start = skip_sql_trivia(query, start + modifier.len());
                if modifier == "DISTINCT"
                    && upper[start..].starts_with("ON")
                    && local_keyword_boundary(upper, start, "ON")
                {
                    let open = skip_sql_trivia(query, start + 2);
                    let Some(close) = matching_parenthesis_in_searchable(upper, open) else {
                        return Output::default();
                    };
                    start = skip_sql_trivia(query, close + 1);
                }
                break;
            }
        }
        let end = [
            "FROM", "WHERE", "GROUP", "HAVING", "WINDOW", "QUALIFY", "ORDER", "LIMIT", "OFFSET",
            "FETCH", "FOR",
        ]
        .iter()
        .filter_map(|keyword| find_top_level_keyword_in_searchable(upper, start, keyword))
        .min()
        .unwrap_or(query.len());
        let augmented = projection::row_source_schema(
            query,
            upper,
            self.schema,
            self.uri,
            start,
            self.line,
            self.dialect,
        );
        let base = augmented.as_ref().unwrap_or(self.schema);
        let derived = derived_tables_with_inference(
            query,
            upper,
            base,
            self.uri,
            start,
            |body, schema, line, names| {
                let context = Context {
                    schema,
                    uri: self.uri,
                    line: self.line.saturating_add(line.saturating_sub(1)),
                    dialect: self.dialect,
                    environment: self.environment,
                };
                let mask = SqlParser::mask_sql_noise(body).to_ascii_uppercase();
                let output = context.query(body, &mask, depth + 1, budget);
                context.finish(output, names)
            },
        );
        let derived_names: Vec<_> = derived.iter().map(|table| table.name.clone()).collect();
        let derived_schema = (!derived.is_empty()).then(|| {
            let mut schema = base.clone();
            for table in derived {
                install_local_table(&mut schema, table);
            }
            schema
        });
        let schema = derived_schema.as_ref().unwrap_or(base);
        let views = super::lateral_views::tables(
            query,
            schema,
            self.uri,
            start,
            self.dialect,
            self.environment,
        );
        let view_schema = (!views.is_empty()).then(|| {
            let mut schema = schema.clone();
            for table in views {
                install_local_table(&mut schema, table);
            }
            schema
        });
        let schema = view_schema.as_ref().unwrap_or(schema);
        let mut sources = projection::ProjectionSources::new(
            query,
            upper,
            schema,
            start,
            self.dialect,
            &derived_names,
            self.environment,
        );
        let context = Context {
            schema,
            uri: self.uri,
            line: self.line,
            dialect: self.dialect,
            environment: self.environment,
        };
        sources.infer_scalar_outputs(&context, query, upper, start..end, depth, budget);
        let mut output = Output {
            columns: Vec::new(),
            known_arity: true,
        };
        for (start, end) in split_top_level_ranges_in_searchable(upper, start, end) {
            let expression = query[start..end].trim();
            if expression.is_empty() {
                output.known_arity = false;
                continue;
            }
            let star = sources.star_projection(query, start, end).or_else(|| {
                (expression == "*" || expression.ends_with(".*"))
                    .then(|| sources.star(expression.strip_suffix(".*").map(str::trim)))
            });
            if let Some(columns) = star {
                output.known_arity &= !columns.is_empty();
                output.columns.extend(columns);
                continue;
            }
            let name = sources
                .expression_name(query, start, end)
                .or_else(|| select_expression_output_name(expression))
                .unwrap_or_default();
            let ty = sources.expression_type(query, start, end, self.dialect);
            let mut column = local_relation_column(
                name,
                ty.as_ref().map_or("unknown", |ty| ty.data_type.as_str()),
                self.uri,
                self.line,
            );
            column.nullable = ty.is_none_or(|ty| ty.nullable);
            column.comment = sources.expression_comment(query, start, end);
            output.columns.push(column);
        }
        output
    }
}

/// Split at the last lowest-precedence operator: UNION/EXCEPT associate left
/// to right, with INTERSECT binding more tightly. Noise is already masked.
fn set_split(upper: &str) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let mut lower = None;
    let mut intersect = None;
    let bytes = upper.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b'A'..=b'Z' if depth == 0 => {
                let start = index;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'$'))
                {
                    index += 1;
                }
                let word = &upper[start..index];
                if local_keyword_boundary(upper, start, word) {
                    match word {
                        "UNION" | "EXCEPT" => lower = Some((start, word.len())),
                        "INTERSECT" => intersect = Some((start, word.len())),
                        _ => {}
                    }
                }
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    lower.or(intersect)
}
pub(super) fn select_expression_output_name(expression: &str) -> Option<String> {
    let upper = SqlParser::mask_sql_noise(expression).to_ascii_uppercase();
    if let Some(as_offset) = find_top_level_keyword_in_searchable(&upper, 0, "AS") {
        return read_local_identifier_after(expression, as_offset + "AS".len())
            .map(|(name, _, _)| name);
    }
    let trimmed = expression.trim();
    if trimmed
        .chars()
        .all(|character| character.is_alphanumeric() || matches!(character, '_' | '$' | '.'))
    {
        return trimmed.rsplit('.').next().map(str::to_string);
    }
    let trailing = trimmed
        .split_whitespace()
        .next_back()
        .filter(|value| !Keywords::is_keyword(value))?;
    trailing
        .chars()
        .all(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
        .then(|| trailing.to_string())
}
