//! Bounded expression inference over the projection's existing syntax tree.
//! Unsupported functions/types remain unknown; catalog fields stay nullable
//! because a surrounding outer join can null-extend even a NOT NULL column.
mod arithmetic;
mod hive;
mod literals;
mod patterns;
pub(super) use patterns::expected_escape_type as pattern_escape_type;
pub(super) use patterns::expected_value_type as pattern_value_type;
pub(super) use patterns::{continuation as pattern_continuation, PatternContinuation};
mod predicates;
mod quantified;
mod routines;
mod row_values;
mod scalars;
mod search;
mod set_types;
pub(super) use quantified::operand as quantified_operand;
pub(super) use set_types::merge as merge_set_types;
pub(super) const MAX_COMPOUND_ITEMS: usize = 256;
pub(super) const UNTYPED_STRING: &str = "__oxide_untyped_string__";

use super::projection::ProjectionSources;
use tree_sitter::Node;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ExpressionType {
    pub data_type: String,
    pub nullable: bool,
}

impl ExpressionType {
    fn new(data_type: &str, nullable: bool) -> Self {
        Self {
            data_type: data_type.into(),
            nullable,
        }
    }
}

pub(super) fn infer(
    node: Node<'_>,
    sql: &str,
    sources: &ProjectionSources<'_>,
    dialect: Option<&str>,
    depth: usize,
) -> Option<ExpressionType> {
    if depth >= 32 {
        return None;
    }
    if node.has_error() || node.is_missing() {
        return None;
    }
    let dialect = dialect.unwrap_or("");
    let recurse = |child| infer(child, sql, sources, Some(dialect), depth + 1);
    let mut cursor = node.walk();
    match node.kind() {
        "array" if dialect == "hive" => hive::array(node, sql, sources, depth),
        "exists" => predicates::exists(dialect),
        "subquery" => sources.scalar_type(node.start_byte(), node.end_byte()),
        "term" => recurse(node.child_by_field_name("value")?),
        "identifier" | "field"
            if crate::placeholder::SqlPlaceholderDialect::for_name(Some(dialect))
                == crate::placeholder::SqlPlaceholderDialect::SqlServer
                && node.utf8_text(sql.as_bytes()).ok()?.starts_with('@') =>
        {
            sources.variable_type(sql, node.start_byte(), node.utf8_text(sql.as_bytes()).ok()?)
        }
        "field" => Some(ExpressionType::new(
            &sources.column_type(node.utf8_text(sql.as_bytes()).ok()?)?,
            true,
        )),
        "parenthesized_expression" => recurse(node.named_child(0)?),
        "window_function" => recurse(
            node.named_children(&mut cursor)
                .find(|n| n.kind() == "invocation")?,
        ),
        "literal" => literals::infer(node.utf8_text(sql.as_bytes()).ok()?, dialect),
        "binary_expression" => {
            let left_node = node.child_by_field_name("left")?;
            let right_node = node.child_by_field_name("right")?;
            let left = recurse(left_node);
            let operator = crate::parser::SqlParser::source_node_text(
                node.child_by_field_name("operator")?,
                sql,
            )
            .trim();
            if let Some((pattern_node, escape_node)) = patterns::escape_operands(right_node) {
                let pattern = recurse(pattern_node);
                let escape = recurse(escape_node);
                return patterns::escaped(
                    (left.as_ref(), left_node),
                    operator,
                    (pattern.as_ref(), pattern_node),
                    (escape.as_ref(), escape_node),
                    sql,
                    dialect,
                    sources.server_version(),
                );
            }
            let right = recurse(right_node);
            if let Some(result) = patterns::binary(
                (left.as_ref(), left_node),
                operator,
                (right.as_ref(), right_node),
                sql,
                dialect,
                sources.server_version(),
            ) {
                return Some(result);
            }
            if let Some(result) = predicates::binary(
                (left.as_ref(), left_node),
                operator,
                (right.as_ref(), right_node),
                sql,
                sources,
                dialect,
                depth,
            ) {
                return Some(result);
            }
            if let Some(result) = search::binary(
                (left.as_ref(), left_node),
                operator,
                (right.as_ref(), right_node),
                sql,
                dialect,
                sources.server_version(),
            ) {
                return Some(result);
            }
            arithmetic::binary(left?, operator, right?, dialect)
        }
        "unary_expression" => {
            let argument = recurse(node.child_by_field_name("operand")?);
            let operator = node
                .child_by_field_name("operator")?
                .utf8_text(sql.as_bytes())
                .ok()?;
            if let Some(result) = predicates::unary(operator.trim(), argument.as_ref(), dialect) {
                return Some(result);
            }
            let argument = argument?;
            if dialect == "postgres"
                && ((operator.eq_ignore_ascii_case("NOT") && argument.data_type == "boolean")
                    || (operator.trim() == "!!" && argument.data_type == "tsquery"))
            {
                return Some(ExpressionType::new(&argument.data_type, argument.nullable));
            }
            arithmetic::unary(operator.trim(), argument, dialect)
        }
        "cast"
            if matches!(
                dialect,
                "postgres"
                    | "mysql"
                    | "sqlserver"
                    | "mssql"
                    | "tsql"
                    | "hive"
                    | "duckdb"
                    | "clickhouse"
                    | "sqlite"
            ) =>
        {
            let try_cast = sql
                .get(node.start_byte()..node.start_byte() + 8)
                .is_some_and(|name| name.eq_ignore_ascii_case("TRY_CAST"));
            if try_cast && !matches!(dialect, "sqlserver" | "mssql" | "tsql" | "duckdb") {
                return None;
            }
            if node.child_by_field_name("parameter").is_none() {
                // The grammar aliases PostgreSQL postfix casts to the same
                // node kind, but their delimiter and argument have no fields.
                if dialect != "postgres" {
                    return None;
                }
                let argument = node
                    .named_children(&mut node.walk())
                    .find(|child| !child.is_extra())?;
                let mut delimiter_seen = false;
                let mut type_start = None;
                for child in node.children(&mut node.walk()) {
                    if child.kind() == "::" {
                        delimiter_seen = true;
                    } else if delimiter_seen && !child.is_extra() {
                        type_start = Some(child.start_byte());
                        break;
                    }
                }
                let text = sql.get(type_start?..node.end_byte())?.trim();
                return (!text.is_empty()).then(|| {
                    ExpressionType::new(text, recurse(argument).is_none_or(|ty| ty.nullable))
                });
            }
            let argument = node.child_by_field_name("parameter")?;
            // Use the type node, never search for AS inside a nested expression.
            let mut after_as = false;
            let mut type_start = None;
            for child in node.named_children(&mut cursor) {
                if child.kind() == "keyword_as" {
                    after_as = true;
                } else if after_as {
                    type_start = Some(child.start_byte());
                    break;
                }
            }
            let text = sql
                .get(type_start?..node.end_byte().checked_sub(1)?)?
                .trim();
            if text.is_empty() {
                return None;
            }
            let text = if dialect == "sqlite" {
                sqlite_cast_affinity(text)
            } else {
                text
            };
            Some(ExpressionType::new(
                text,
                try_cast || recurse(argument).is_none_or(|ty| ty.nullable),
            ))
        }
        "between_expression" => {
            let left_node = node.child_by_field_name("left")?;
            let low_node = node.child_by_field_name("low")?;
            let high_node = node.child_by_field_name("high")?;
            let left = recurse(left_node);
            let low = recurse(low_node);
            let high = recurse(high_node);
            predicates::between(
                (left.as_ref(), left_node),
                (low.as_ref(), low_node),
                (high.as_ref(), high_node),
                sql,
                sources,
                dialect,
                depth,
            )
        }
        "case"
            if matches!(
                dialect,
                "postgres"
                    | "mysql"
                    | "sqlserver"
                    | "mssql"
                    | "tsql"
                    | "hive"
                    | "duckdb"
                    | "clickhouse"
                    | "sqlite"
            ) =>
        {
            let mut result_next = false;
            let mut has_else = false;
            let mut results = Vec::new();
            for child in node.named_children(&mut cursor) {
                if matches!(child.kind(), "keyword_then" | "keyword_else") {
                    has_else |= child.kind() == "keyword_else";
                    result_next = true;
                } else if result_next {
                    results.push(recurse(child)?);
                    result_next = false;
                }
            }
            let mut ty = if dialect == "postgres" {
                common_postgres_type(&results)?
            } else {
                let mut results = results.into_iter();
                let first = results.next()?;
                results.try_fold(first, |left, right| {
                    merge_set_types(Some(dialect), left, right)
                })?
            };
            ty.nullable |= !has_else;
            Some(ty)
        }
        "invocation" => {
            let identity = routines::reference(node, sql, dialect)?;
            let name = identity.name().to_ascii_lowercase();
            let args: Vec<_> = node
                .named_children(&mut node.walk())
                .filter(|n| n.kind() == "term")
                .collect();
            if name == "match"
                && dialect == "mysql"
                && sources.is_mysql_match(node.start_byte(), node.end_byte())
            {
                return Some(ExpressionType::new("double", true));
            }
            if let Some(result) = routines::resolve(node, sql, sources, &identity, dialect, depth) {
                return result;
            }
            if !identity.permits_builtin(dialect) || !identity.matches_builtin(&name, dialect) {
                return None;
            }
            let pattern_name = if dialect == "clickhouse" {
                identity.name()
            } else {
                &name
            };
            if let Some(result) = patterns::call(pattern_name, node, sql, sources, dialect, depth) {
                return Some(result);
            }
            if let Some(result) = scalars::infer_builtin(&name, &args, sql, sources, dialect, depth)
            {
                return Some(result);
            }
            if dialect == "hive" {
                if let Some(ty) = hive::constructor(&name, &args, sql, sources, depth) {
                    return Some(ty);
                }
            }
            if name != "match" {
                if let Some(result) = crate::search_functions::return_type(
                    dialect,
                    &name,
                    args.len(),
                    sources.server_version(),
                ) {
                    return Some(ExpressionType::new(result, true));
                }
            }
            if dialect == "clickhouse" && name == "count" && args.len() <= 1 {
                return Some(ExpressionType::new("UInt64", false));
            }
            if name == "count" && args.len() == 1 {
                return match dialect {
                    "postgres" | "mysql" => Some(ExpressionType::new("bigint", false)),
                    "sqlite" => Some(ExpressionType::new("integer", false)),
                    "sqlserver" | "mssql" | "tsql" => Some(ExpressionType::new("int", true)),
                    _ => None,
                };
            }
            if dialect == "clickhouse"
                && args.is_empty()
                && matches!(
                    identity.name(),
                    "row_number" | "rank" | "dense_rank" | "denseRank"
                )
            {
                return Some(ExpressionType::new("UInt64", false));
            }
            if dialect == "postgres"
                && matches!(name.as_str(), "row_number" | "rank" | "dense_rank")
                && args.is_empty()
            {
                return Some(ExpressionType::new("bigint", false));
            }
            if matches!(dialect, "sqlserver" | "mssql" | "tsql")
                && name == "coalesce"
                && !args.is_empty()
            {
                let mut values = args.iter().map(|node| recurse(*node));
                let first = values.next()??;
                let mut result = values.try_fold(first, |left, right| {
                    merge_set_types(Some(dialect), left, right?)
                })?;
                result.nullable = true;
                return Some(result);
            }
            if dialect == "postgres" && name == "coalesce" && !args.is_empty() {
                let values: Vec<_> = args.into_iter().map(recurse).collect::<Option<_>>()?;
                let mut ty = common_postgres_type(&values)?;
                ty.nullable = values.iter().all(|value| value.nullable);
                return Some(ty);
            }
            if args.len() != 1 {
                return None;
            }
            let argument = recurse(args[0])?;
            if matches!(name.as_str(), "sum" | "avg") {
                let ty = aggregate_type(dialect, &name, &argument.data_type)?;
                return Some(ExpressionType::new(ty, true));
            }
            if dialect == "postgres"
                && matches!(name.as_str(), "min" | "max")
                && builtin_ordered_type(&argument.data_type)
            {
                return Some(ExpressionType::new(&argument.data_type, true));
            }
            None
        }
        _ => None,
    }
}

fn sqlite_cast_affinity(type_name: &str) -> &'static str {
    let name = type_name.to_ascii_uppercase();
    if name.contains("INT") {
        "integer"
    } else if ["CHAR", "CLOB", "TEXT"]
        .iter()
        .any(|part| name.contains(part))
    {
        "text"
    } else if name.contains("BLOB") || name.trim_matches(['\"', '`', '[', ']']).is_empty() {
        "blob"
    } else if ["REAL", "FLOA", "DOUB"]
        .iter()
        .any(|part| name.contains(part))
    {
        "real"
    } else {
        "numeric"
    }
}

fn numeric_base(ty: &str) -> &str {
    ty.split('(').next().unwrap_or(ty).trim()
}

fn aggregate_type<'a>(dialect: &str, name: &str, argument: &'a str) -> Option<&'a str> {
    let postgres = crate::postgres_types::builtin_type(argument);
    let ty = if dialect == "postgres" {
        let ty = postgres.as_ref()?;
        if ty.array {
            return None;
        }
        ty.name
    } else {
        numeric_base(argument)
    };
    match dialect {
        "postgres" => match ty.to_ascii_lowercase().as_str() {
            "smallint" | "int2" | "integer" | "int" | "int4" if name == "sum" => Some("bigint"),
            "smallint" | "int2" | "integer" | "int" | "int4" | "bigint" | "int8" | "numeric"
            | "decimal" => Some("numeric"),
            "real" | "float4" if name == "sum" => Some("real"),
            "real" | "float4" | "double precision" | "float8" => Some("double precision"),
            "interval" => Some("interval"),
            "money" if name == "sum" => Some("money"),
            _ => None,
        },
        "mysql" => match ty.to_ascii_lowercase().as_str() {
            "tinyint" | "smallint" | "mediumint" | "int" | "integer" | "bigint"
            | "bigint unsigned" | "decimal" | "numeric" => Some("decimal"),
            "float" | "double" | "double precision" | "real" => Some("double"),
            _ => None,
        },
        _ => None,
    }
}

fn builtin_ordered_type(ty: &str) -> bool {
    let Some(ty) = crate::postgres_types::builtin_type(ty) else {
        return false;
    };
    if ty.array {
        return false;
    }
    matches!(
        ty.name,
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
            | "date"
            | "timestamp"
            | "timestamptz"
    )
}

pub(super) fn common_postgres_type(values: &[ExpressionType]) -> Option<ExpressionType> {
    let types: Vec<_> = values
        .iter()
        .filter(|ty| ty.data_type != "null" && ty.data_type != UNTYPED_STRING)
        .collect();
    if types.is_empty() && values.iter().any(|ty| ty.data_type == UNTYPED_STRING) {
        return Some(ExpressionType::new(
            "text",
            values.iter().any(|ty| ty.nullable),
        ));
    }
    let first = types.first()?;
    let first_builtin = crate::postgres_types::builtin_type(&first.data_type);
    let same_builtin = first_builtin.as_ref().filter(|first| {
        types.iter().all(|ty| {
            crate::postgres_types::builtin_type(&ty.data_type)
                .is_some_and(|ty| ty.name == first.name && ty.array == first.array)
        })
    });
    let data_type = if types.iter().all(|ty| ty.data_type == first.data_type) {
        first.data_type.clone()
    } else if let Some(ty) = same_builtin {
        format!("{}{}", ty.name, if ty.array { "[]" } else { "" })
    } else if types.iter().all(|ty| {
        ty.data_type == first.data_type
            || (builtin_ordered_type(&first.data_type)
                && ty.data_type.eq_ignore_ascii_case(&first.data_type))
    }) {
        first.data_type.clone()
    } else {
        // Only the unambiguous integer/numeric ladder is modeled here. Domains,
        // text coercions and incompatible branches require catalog resolution.
        let ranks: Vec<_> = types
            .iter()
            .map(|ty| arithmetic::pg_rank(&ty.data_type))
            .collect::<Option<_>>()?;
        [
            "smallint",
            "integer",
            "bigint",
            "numeric",
            "real",
            "double precision",
        ][*ranks.iter().max()?]
        .to_string()
    };
    Some(ExpressionType {
        data_type,
        nullable: values.iter().any(|ty| ty.nullable),
    })
}

#[cfg(test)]
mod tests;
