//! Signature ranking reuses projection semantics and one set of input bindings.
use super::{
    projection::ProjectionSources,
    provenance::ParsedOrigin,
    query_output::{Context, MAX_SET_BRANCHES},
};
use crate::{parser::SqlParser, schema::Schema};
use std::ops::Range;

#[derive(Default)]
pub(in crate::server) struct ArgumentTypes {
    pub(in crate::server) types: Vec<Option<String>>,
    pub(in crate::server) complete_arity: Option<usize>,
    pub(in crate::server) explicit_variadic: bool,
    pattern_continuation: Option<super::expression_types::PatternContinuation>,
}

pub(in crate::server) struct ContinuationType {
    pub(in crate::server) logical: Option<bool>,
    pub(in crate::server) escape: Option<bool>,
}
pub(in crate::server) fn infer_continuation_type(
    text: &str,
    operator: Range<usize>,
    schema: &Schema,
    cursor: usize,
    dialect: &str,
) -> ContinuationType {
    let result = infer_types(
        text,
        Probe::Continuation(operator.start, operator.end),
        schema,
        "",
        cursor,
        dialect,
        None,
    );
    ContinuationType {
        logical: result
            .types
            .into_iter()
            .next()
            .flatten()
            .and_then(|ty| {
                crate::dialects::common::type_supports_logical_continuation(&ty, dialect)
            })
            .or_else(|| result.pattern_continuation.map(|_| true)),
        escape: result
            .pattern_continuation
            .and_then(|pattern| pattern.escape),
    }
}

pub(in crate::server) fn infer_routine_argument_types(
    text: &str,
    ranges: &[Range<usize>],
    schema: &Schema,
    uri: &str,
    cursor: usize,
    dialect: &str,
    routines: Option<&crate::schema::SchemaManager>,
) -> ArgumentTypes {
    infer_types(
        text,
        Probe::Arguments(ranges),
        schema,
        uri,
        cursor,
        dialect,
        routines,
    )
}

pub(in crate::server) fn infer_comparison_left_type(
    text: &str,
    operator: Range<usize>,
    schema: &Schema,
    uri: &str,
    cursor: usize,
    dialect: &str,
    routines: Option<&crate::schema::SchemaManager>,
) -> Option<String> {
    infer_types(
        text,
        Probe::Comparison(operator.start, operator.end, false),
        schema,
        uri,
        cursor,
        dialect,
        routines,
    )
    .types
    .into_iter()
    .next()
    .flatten()
}

pub(in crate::server) fn infer_comparison_value_type(
    text: &str,
    operator: Range<usize>,
    schema: &Schema,
    uri: &str,
    cursor: usize,
    dialect: &str,
    routines: Option<&crate::schema::SchemaManager>,
) -> Option<String> {
    infer_types(
        text,
        Probe::Comparison(operator.start, operator.end, true),
        schema,
        uri,
        cursor,
        dialect,
        routines,
    )
    .types
    .into_iter()
    .next()
    .flatten()
}

#[derive(Clone, Copy)]
enum Probe<'a> {
    Arguments(&'a [Range<usize>]),
    Comparison(usize, usize, bool),
    Continuation(usize, usize),
}

fn infer_types(
    text: &str,
    probe: Probe<'_>,
    schema: &Schema,
    uri: &str,
    cursor: usize,
    dialect: &str,
    routines: Option<&crate::schema::SchemaManager>,
) -> ArgumentTypes {
    let ranges = match probe {
        Probe::Arguments(ranges) => ranges,
        Probe::Comparison(offset, end, _) | Probe::Continuation(offset, end) => {
            if offset >= end
                || end > text.len()
                || !text.is_char_boundary(offset)
                || !text.is_char_boundary(end)
            {
                return ArgumentTypes::default();
            }
            &[]
        }
    };
    if text.len() > super::LOCAL_RELATION_SCAN_MAX_BYTES
        || matches!(probe, Probe::Arguments(_)) && ranges.is_empty()
        || ranges.len() > 256
    {
        return ArgumentTypes::default();
    }
    let dialect = match crate::placeholder::SqlPlaceholderDialect::for_name(Some(dialect)) {
        crate::placeholder::SqlPlaceholderDialect::Postgres => "postgres",
        crate::placeholder::SqlPlaceholderDialect::Mysql => "mysql",
        crate::placeholder::SqlPlaceholderDialect::SqlServer => "sqlserver",
        _ => dialect,
    };
    let upper = SqlParser::mask_sql_noise_for_dialect(text, dialect).to_ascii_uppercase();
    let content = SqlParser::mask_sql_operand_context_for_dialect(text, dialect);
    let parsed = match probe {
        Probe::Arguments(_) => ParsedOrigin::new(text, Some(dialect)),
        Probe::Comparison(start, end, _) | Probe::Continuation(start, end) => {
            ParsedOrigin::for_comparison(text, Some(dialect), start..end)
        }
    };
    let origin = parsed.view();
    let variables = super::variables::Snapshot::new(text, cursor, Some(dialect), parsed.tree());
    let variables = variables
        .as_ref()
        .and_then(|snapshot| snapshot.origin(text));
    let context = Context {
        schema,
        uri,
        line: 1,
        dialect: Some(dialect),
        environment: super::query_output::Environment {
            origin,
            routines,
            variables,
        },
    };
    let mut sources = ProjectionSources::new(
        text,
        &upper,
        schema,
        cursor,
        Some(dialect),
        &[],
        super::query_output::Environment {
            origin,
            routines,
            variables,
        },
    );
    let mut comparison_operator = None;
    let complete = match probe {
        Probe::Arguments(_) => ranges
            .first()
            .and_then(|range| range.start.checked_sub(1))
            .and_then(|opening| sources.invocation_arguments(opening)),
        Probe::Comparison(operator, _, _) | Probe::Continuation(operator, _) => {
            match sources.comparison_ranges(operator) {
                Some((range, op)) => {
                    comparison_operator = Some(op);
                    Some((vec![range], false))
                }
                None => return ArgumentTypes::default(),
            }
        }
    };
    let complete_arity = complete.as_ref().map(|(ranges, _)| ranges.len());
    let explicit_variadic = complete.as_ref().is_some_and(|(_, explicit)| *explicit);
    let ranges = complete
        .as_ref()
        .map(|(ranges, _)| ranges.as_slice())
        .unwrap_or(ranges);
    let mut budget = MAX_SET_BRANCHES;
    if let (Some(first), Some(last)) = (ranges.first(), ranges.last()) {
        sources.infer_scalar_outputs(
            &context,
            text,
            &upper,
            first.start..last.end,
            0,
            &mut budget,
        );
    }
    let mut pattern_continuation = None;
    let types = ranges
        .iter()
        .enumerate()
        .map(|(index, range)| {
            if complete_arity.is_none()
                && index + 1 == ranges.len()
                && content
                    .get(range.end..)?
                    .trim_start()
                    .chars()
                    .next()
                    .is_some_and(|ch| !matches!(ch, ',' | ')'))
            {
                return None;
            }
            let value = match probe {
                Probe::Arguments(_) => {
                    crate::server::routine_arguments::argument(text, range.clone(), dialect).value
                }
                Probe::Comparison(_, _, _) | Probe::Continuation(_,_) => range.clone(),
            };
            if matches!(probe,Probe::Continuation(_,_)){
                pattern_continuation=sources.continuation_pattern(text,&content,value.clone(),dialect);
            }
            let ty = sources
                .argument_type(text, &content, value, Some(dialect))
                // Ranking only recognizes short builtin identities. At most
                // 256 names of 256 bytes are retained, even for huge metadata.
                .filter(|ty| ty.data_type.len() <= 256);
            if matches!(probe, Probe::Comparison(_, _, true)) {
                if matches!(probe,Probe::Comparison(start,end,_) if text.get(start..end).is_some_and(|value|value.eq_ignore_ascii_case("ESCAPE"))){
                    return super::expression_types::pattern_escape_type(text.get(comparison_operator.clone()?)?,dialect,sources.server_version(),ty.as_ref()).flatten().map(str::to_owned);
                }
                if let Some(expected) = super::expression_types::pattern_value_type(
                    text.get(comparison_operator.clone()?)?,
                    dialect,
                    sources.server_version(),
                    ty.as_ref(),
                ) {
                    return expected.map(str::to_owned);
                }
            }
            ty.map(|ty| ty.data_type)
        })
        .collect();
    ArgumentTypes {
        types,
        complete_arity,
        explicit_variadic,
        pattern_continuation,
    }
}
