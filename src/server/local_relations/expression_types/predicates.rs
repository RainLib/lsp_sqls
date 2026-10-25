//! Predicate result types are dialect-specific. Fixed SQL predicates do not
//! borrow the selected-column type of EXISTS/IN queries. PostgreSQL's extensible
//! ordinary comparison operators still require known built-in operands.
use super::{ExpressionType, UNTYPED_STRING};
use crate::server::local_relations::projection::ProjectionSources;
use tree_sitter::Node;

pub(super) fn result(dialect: &str, nullable: bool) -> Option<ExpressionType> {
    let ty = match dialect {
        "postgres" | "duckdb" | "hive" => "boolean",
        "mysql" => "bigint",
        "sqlite" => "integer",
        "clickhouse" => "UInt8",
        _ => return None,
    };
    Some(ExpressionType::new(ty, nullable))
}

pub(super) fn exists(dialect: &str) -> Option<ExpressionType> {
    result(dialect, false)
}

pub(super) fn binary(
    left: (Option<&ExpressionType>, Node<'_>),
    operator: &str,
    right: (Option<&ExpressionType>, Node<'_>),
    sql: &str,
    sources: &ProjectionSources<'_>,
    dialect: &str,
    depth: usize,
) -> Option<ExpressionType> {
    result(dialect, true)?;
    let operator = crate::parser::SqlParser::mask_sql_noise(operator)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase();
    let left_null = nullable(left.0, left.1, sql);
    let right_null = nullable(right.0, right.1, sql);
    if super::quantified::operand(right.1).is_some() {
        let comparison = matches!(
            operator.as_str(),
            "=" | "<>" | "!=" | "<" | ">" | "<=" | ">="
        );
        let search = dialect == "postgres"
            && matches!(
                operator.as_str(),
                "LIKE"
                    | "NOT LIKE"
                    | "ILIKE"
                    | "NOT ILIKE"
                    | "~"
                    | "~*"
                    | "!~"
                    | "!~*"
                    | "@@"
                    | "@?"
                    | "@>"
                    | "<@"
                    | "?"
                    | "?|"
                    | "?&"
            );
        return (comparison || search)
            .then(|| {
                super::quantified::infer(left, right.1, &operator, sql, sources, dialect, depth)
            })
            .flatten();
    }
    match operator.as_str() {
        "IN" | "NOT IN" => {
            // ClickHouse IN/NOT IN produce 0/1, including nullable inputs.
            // transform_null_in changes equality, not result nullability.
            if dialect == "clickhouse" {
                return result(dialect, false);
            }
            if right.1.kind() == "subquery" {
                return result(dialect, true);
            }
            if right.1.kind() != "list" {
                return None;
            }
            let mut walk = right.1.walk();
            let mut values = right
                .1
                .named_children(&mut walk)
                .filter(|node| !node.is_extra());
            let Some(first) = values.next() else {
                return (dialect == "sqlite")
                    .then(|| result(dialect, false))
                    .flatten();
            };
            if left_null {
                return result(dialect, true);
            }
            for (index, value) in std::iter::once(first).chain(values).enumerate() {
                // Cap nullability work, keeping the fixed predicate type. A
                // larger or unresolved list conservatively remains nullable.
                if index >= super::MAX_COMPOUND_ITEMS {
                    return result(dialect, true);
                }
                let ty = super::infer(value, sql, sources, Some(dialect), depth + 1);
                if nullable(ty.as_ref(), value, sql) {
                    return result(dialect, true);
                }
            }
            result(dialect, false)
        }
        "IS" | "IS NOT" => {
            let right_text = right.1.utf8_text(sql.as_bytes()).ok()?.trim();
            if dialect == "sqlite" || right_text.eq_ignore_ascii_case("NULL") {
                return result(dialect, false);
            }
            if ["TRUE", "FALSE", "UNKNOWN"]
                .iter()
                .any(|word| right_text.eq_ignore_ascii_case(word))
                && (dialect != "postgres" || pg_truth(left.0))
                && dialect != "clickhouse"
                && !(dialect == "hive" && right_text.eq_ignore_ascii_case("UNKNOWN"))
                && (dialect != "hive" || logical_operand(left.0, dialect))
            {
                return result(dialect, false);
            }
            None
        }
        "IS DISTINCT FROM" | "IS NOT DISTINCT FROM" if matches!(dialect, "postgres" | "duckdb") => {
            result(dialect, false)
        }
        "<=>" if matches!(dialect, "mysql" | "hive") => result(dialect, false),
        "AND" | "OR"
            if (dialect != "postgres" || pg_truth(left.0) && pg_truth(right.0))
                && logical_operand(left.0, dialect)
                && logical_operand(right.0, dialect) =>
        {
            result(dialect, left_null || right_null)
        }
        "=" | "<>" | "!=" | "<" | ">" | "<=" | ">=" => {
            if super::row_values::is_row(left.1, sql, sources, dialect)
                || super::row_values::is_row(right.1, sql, sources, dialect)
            {
                return super::row_values::compare(left.1, right.1, sql, sources, dialect, depth);
            }
            (dialect != "postgres" || pg_comparable(left.0, right.0))
                .then(|| result(dialect, left_null || right_null))
                .flatten()
        }
        _ => None,
    }
}

pub(super) fn unary(
    operator: &str,
    argument: Option<&ExpressionType>,
    dialect: &str,
) -> Option<ExpressionType> {
    if (operator.eq_ignore_ascii_case("NOT") || operator == "!" && dialect == "mysql")
        && (dialect != "postgres" || pg_truth(argument))
        && logical_operand(argument, dialect)
    {
        result(dialect, argument.is_none_or(|ty| ty.nullable))
    } else {
        None
    }
}

fn logical_operand(argument: Option<&ExpressionType>, dialect: &str) -> bool {
    !matches!(dialect, "duckdb" | "clickhouse" | "hive")
        || argument.is_none_or(|ty| {
            ty.data_type.eq_ignore_ascii_case("null")
                || crate::dialects::common::type_supports_logical_continuation(
                    &ty.data_type,
                    dialect,
                ) != Some(false)
        })
}

pub(super) fn between(
    left: (Option<&ExpressionType>, Node<'_>),
    low: (Option<&ExpressionType>, Node<'_>),
    high: (Option<&ExpressionType>, Node<'_>),
    sql: &str,
    sources: &ProjectionSources<'_>,
    dialect: &str,
    depth: usize,
) -> Option<ExpressionType> {
    if [left.1, low.1, high.1]
        .iter()
        .any(|node| super::row_values::is_row(*node, sql, sources, dialect))
    {
        let lower = super::row_values::compare(left.1, low.1, sql, sources, dialect, depth)?;
        let upper = super::row_values::compare(left.1, high.1, sql, sources, dialect, depth)?;
        return result(dialect, lower.nullable || upper.nullable);
    }
    if dialect == "postgres" && (!pg_comparable(left.0, low.0) || !pg_comparable(left.0, high.0)) {
        return None;
    }
    result(
        dialect,
        [left.0, low.0, high.0]
            .iter()
            .any(|value| value.is_none_or(|ty| ty.nullable)),
    )
}

pub(super) fn nullable(ty: Option<&ExpressionType>, node: Node<'_>, sql: &str) -> bool {
    if let Some(ty) = ty {
        return ty.nullable;
    }
    if node.kind() == "literal" {
        if let Ok(text) = node.utf8_text(sql.as_bytes()) {
            if text.starts_with('\'') || text.parse::<f64>().is_ok() {
                return false;
            }
        }
    }
    true
}
fn pg_truth(ty: Option<&ExpressionType>) -> bool {
    ty.is_none_or(|ty| {
        matches!(ty.data_type.as_str(), "null" | UNTYPED_STRING)
            || crate::postgres_types::builtin_type(&ty.data_type)
                .is_some_and(|ty| !ty.array && ty.name == "boolean")
    })
}
pub(super) fn pg_text(ty: Option<&ExpressionType>) -> bool {
    ty.is_some_and(|ty| {
        matches!(
            crate::postgres_types::builtin_type(&ty.data_type)
                .filter(|ty| !ty.array)
                .map(|ty| ty.name),
            Some("text" | "varchar" | "character")
        ) || matches!(ty.data_type.as_str(), "null" | UNTYPED_STRING)
    })
}
pub(super) fn pg_comparable(left: Option<&ExpressionType>, right: Option<&ExpressionType>) -> bool {
    let (Some(left), Some(right)) = (left, right) else {
        return false;
    };
    let builtin = |ty: &ExpressionType| {
        super::arithmetic::pg_rank(&ty.data_type).is_some()
            || pg_text(Some(ty))
            || matches!(
                crate::postgres_types::builtin_type(&ty.data_type)
                    .filter(|ty| !ty.array)
                    .map(|ty| ty.name),
                Some(
                    "boolean"
                        | "uuid"
                        | "date"
                        | "timestamp"
                        | "timestamp without time zone"
                        | "timestamptz"
                        | "timestamp with time zone"
                        | "bytea"
                )
            )
    };
    if left.data_type == "null" || left.data_type == UNTYPED_STRING {
        return builtin(right);
    }
    if right.data_type == "null" || right.data_type == UNTYPED_STRING {
        return builtin(left);
    }
    super::arithmetic::pg_rank(&left.data_type).is_some()
        && super::arithmetic::pg_rank(&right.data_type).is_some()
        || pg_text(Some(left)) && pg_text(Some(right))
        || crate::server::routine_overloads::canonical_type(&left.data_type, "postgres")
            .zip(crate::server::routine_overloads::canonical_type(
                &right.data_type,
                "postgres",
            ))
            .is_some_and(|(left, right)| left == right)
            && builtin(left)
}
