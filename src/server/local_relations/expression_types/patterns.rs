//! Pattern operators and fixed regex results share operand and nullability rules.
//! This models result types; it never evaluates a regex or invents extensions.
use super::{infer, literals, predicates, ExpressionType};
use crate::server::local_relations::projection::ProjectionSources;
use tree_sitter::Node;

fn text_operand(ty: Option<&ExpressionType>, node: Node<'_>, sql: &str, dialect: &str) -> bool {
    if literals::string_literal(node, sql, dialect) {
        return true;
    }
    let node = literals::operand(node);
    if matches!(dialect, "postgres" | "duckdb" | "clickhouse")
        && (matches!(node.kind(), "array" | "interval")
            || node.kind() == "literal"
                && !node
                    .utf8_text(sql.as_bytes())
                    .is_ok_and(|text| text.eq_ignore_ascii_case("NULL")))
    {
        return false;
    }
    text_type(ty, dialect)
}
fn text_type(ty: Option<&ExpressionType>, dialect: &str) -> bool {
    let Some(ty) = ty else {
        return dialect != "postgres";
    };
    if ty.data_type.len() > 1024 {
        return false;
    }
    if ty.data_type.eq_ignore_ascii_case("null") {
        return true;
    }
    if dialect == "postgres" {
        return predicates::pg_text(Some(ty));
    }
    if matches!(dialect, "mysql" | "sqlite") {
        return crate::dialects::common::type_supports_logical_continuation(&ty.data_type, dialect)
            == Some(true);
    }
    let mut name = ty.data_type.trim().to_ascii_lowercase();
    for _ in 0..4 {
        if (name.starts_with("nullable(") || name.starts_with("lowcardinality("))
            && name.ends_with(')')
        {
            name = name[name.find('(').unwrap() + 1..name.len() - 1]
                .trim()
                .into();
        } else {
            break;
        }
    }
    let base = name.split('(').next().unwrap_or("").trim();
    match dialect {
        "duckdb" | "sqlserver" => matches!(
            base,
            "varchar"
                | "nvarchar"
                | "text"
                | "string"
                | "char"
                | "nchar"
                | "character"
                | "character varying"
        ),
        "hive" => matches!(base, "string" | "varchar" | "char"),
        "clickhouse" => matches!(base, "string" | "fixedstring"),
        _ => false,
    }
}

fn normalized_operator(operator: &str) -> Option<String> {
    // Ordinary arithmetic/comparisons never need a pattern noise scan or allocation.
    if !matches!(
        operator
            .trim_start()
            .as_bytes()
            .first()?
            .to_ascii_uppercase(),
        b'L' | b'I' | b'N' | b'S' | b'R' | b'G' | b'~' | b'!'
    ) {
        return None;
    }
    Some(
        crate::parser::SqlParser::mask_sql_noise(operator)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_uppercase(),
    )
}
fn product_expected_type(
    operator: &str,
    dialect: &str,
    version: Option<(u32, u32)>,
) -> Option<Option<&'static str>> {
    let allowed = match operator {
        "LIKE" | "NOT LIKE" => matches!(
            dialect,
            "postgres" | "mysql" | "sqlite" | "hive" | "duckdb" | "clickhouse" | "sqlserver"
        ),
        "ILIKE" | "NOT ILIKE" => {
            matches!(dialect, "postgres" | "duckdb")
                || dialect == "clickhouse" && version.is_none_or(|v| v >= (20, 6))
        }
        "SIMILAR TO" | "NOT SIMILAR TO" => {
            matches!(dialect, "postgres" | "duckdb")
                || dialect == "clickhouse" && version.is_some_and(|v| v >= (26, 10))
        }
        "REGEXP" => matches!(dialect, "mysql" | "hive" | "clickhouse"),
        "NOT REGEXP" => matches!(dialect, "mysql" | "hive"),
        "RLIKE" | "NOT RLIKE" => matches!(dialect, "mysql" | "hive"),
        "GLOB" => matches!(dialect, "sqlite" | "duckdb"),
        "NOT GLOB" => dialect == "sqlite",
        "~" => matches!(dialect, "postgres" | "duckdb" | "clickhouse"),
        "!~" => {
            matches!(dialect, "postgres" | "duckdb")
                || dialect == "clickhouse" && version.is_some_and(|v| v >= (26, 8))
        }
        "~*" | "!~*" => {
            dialect == "postgres"
                || dialect == "duckdb" && version.is_some_and(|v| v >= (2, 0))
                || dialect == "clickhouse" && version.is_some_and(|v| v >= (26, 8))
        }
        _ => return None,
    };
    Some(allowed.then_some(match dialect {
        "hive" => "string",
        "clickhouse" => "String",
        "duckdb" | "mysql" => "varchar",
        "sqlserver" => "nvarchar",
        _ => "text",
    }))
}
fn expected_type(
    operator: &str,
    dialect: &str,
    version: Option<(u32, u32)>,
) -> Option<Option<&'static str>> {
    product_expected_type(&normalized_operator(operator)?, dialect, version)
}
fn escape_capability(operator: &str, dialect: &str, version: Option<(u32, u32)>) -> Option<bool> {
    if dialect == "clickhouse"
        && matches!(
            operator,
            "LIKE" | "NOT LIKE" | "ILIKE" | "NOT ILIKE" | "SIMILAR TO" | "NOT SIMILAR TO"
        )
    {
        let floor = if operator.contains("SIMILAR") {
            (26, 10)
        } else {
            (26, 6)
        };
        return version.map(|value| value >= floor);
    }
    Some(match operator {
        "LIKE" | "NOT LIKE" | "ILIKE" | "NOT ILIKE" => {
            matches!(
                dialect,
                "postgres" | "mysql" | "sqlite" | "duckdb" | "sqlserver"
            )
        }
        "SIMILAR TO" | "NOT SIMILAR TO" => dialect == "postgres",
        _ => false,
    })
}
pub(in crate::server::local_relations) fn expected_escape_type(
    operator: &str,
    dialect: &str,
    version: Option<(u32, u32)>,
    left: Option<&ExpressionType>,
) -> Option<Option<&'static str>> {
    let operator = normalized_operator(operator)?;
    let supported = escape_capability(&operator, dialect, version) == Some(true);
    product_expected_type(&operator, dialect, version)
        .map(|ty| ty.filter(|_| supported && text_type(left, dialect)))
}
fn escaped_nullable(
    left: (Option<&ExpressionType>, Node<'_>),
    operator: &str,
    right: (Option<&ExpressionType>, Node<'_>),
    escape: (Option<&ExpressionType>, Node<'_>),
    sql: &str,
    dialect: &str,
    version: Option<(u32, u32)>,
) -> Option<bool> {
    expected_escape_type(operator, dialect, version, left.0)??;
    if !text_operand(escape.0, escape.1, sql, dialect) {
        return None;
    }
    let raw = literals::operand(escape.1).utf8_text(sql.as_bytes()).ok()?;
    if let Some(value) = raw.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        if !value.contains('\\') {
            let value = value.replace("''", "'");
            let length = value.chars().count();
            if length > 1
                || length == 0 && !matches!(dialect, "postgres" | "duckdb" | "mysql")
                || matches!(dialect, "clickhouse" | "duckdb") && !value.is_ascii()
            {
                return None;
            }
        }
    }
    Some(
        binary_nullable(left, operator, right, sql, dialect, version)?
            || predicates::nullable(escape.0, literals::operand(escape.1), sql),
    )
}
pub(super) fn escaped(
    left: (Option<&ExpressionType>, Node<'_>),
    operator: &str,
    right: (Option<&ExpressionType>, Node<'_>),
    escape: (Option<&ExpressionType>, Node<'_>),
    sql: &str,
    dialect: &str,
    version: Option<(u32, u32)>,
) -> Option<ExpressionType> {
    predicates::result(
        dialect,
        escaped_nullable(left, operator, right, escape, sql, dialect, version)?,
    )
}
pub(super) fn escape_operands(node: Node<'_>) -> Option<(Node<'_>, Node<'_>)> {
    (node.kind() == "binary_expression"
        && node.child_by_field_name("operator")?.kind() == "keyword_escape")
        .then_some(())?;
    Some((
        node.child_by_field_name("left")?,
        node.child_by_field_name("right")?,
    ))
}
pub(in crate::server::local_relations) fn expected_value_type(
    operator: &str,
    dialect: &str,
    version: Option<(u32, u32)>,
    left: Option<&ExpressionType>,
) -> Option<Option<&'static str>> {
    expected_type(operator, dialect, version).map(|ty| ty.filter(|_| text_type(left, dialect)))
}
fn binary_nullable(
    left: (Option<&ExpressionType>, Node<'_>),
    operator: &str,
    right: (Option<&ExpressionType>, Node<'_>),
    sql: &str,
    dialect: &str,
    version: Option<(u32, u32)>,
) -> Option<bool> {
    expected_type(operator, dialect, version)??;
    if !text_operand(left.0, left.1, sql, dialect) || !text_operand(right.0, right.1, sql, dialect)
    {
        return None;
    }
    Some(
        predicates::nullable(left.0, literals::operand(left.1), sql)
            || predicates::nullable(right.0, literals::operand(right.1), sql),
    )
}
pub(super) fn binary(
    left: (Option<&ExpressionType>, Node<'_>),
    operator: &str,
    right: (Option<&ExpressionType>, Node<'_>),
    sql: &str,
    dialect: &str,
    version: Option<(u32, u32)>,
) -> Option<ExpressionType> {
    predicates::result(
        dialect,
        binary_nullable(left, operator, right, sql, dialect, version)?,
    )
}

/// A known matching predicate can continue logically even in SQL Server,
/// where predicates do not have a projected scalar Boolean type. The Boolean
/// capability distinguishes supported, denied and version-unknown ESCAPE.
#[derive(Clone, Copy)]
pub(in crate::server::local_relations) struct PatternContinuation {
    pub(in crate::server::local_relations) escape: Option<bool>,
}
pub(in crate::server::local_relations) fn continuation(
    mut node: Node<'_>,
    sql: &str,
    sources: &ProjectionSources<'_>,
    dialect: &str,
) -> Option<PatternContinuation> {
    for depth in 0..32 {
        if node.has_error() || node.is_missing() {
            return None;
        }
        if node.kind() == "term" {
            node = node.child_by_field_name("value")?;
            continue;
        }
        if node.kind() == "unary_expression"
            && node
                .child_by_field_name("operator")?
                .utf8_text(sql.as_bytes())
                .ok()?
                .eq_ignore_ascii_case("NOT")
        {
            node = node.child_by_field_name("operand")?;
            continue;
        }
        if node.kind() != "binary_expression" {
            return None;
        }
        let operator = node
            .child_by_field_name("operator")?
            .utf8_text(sql.as_bytes())
            .ok()?;
        if operator.eq_ignore_ascii_case("AND") || operator.eq_ignore_ascii_case("OR") {
            node = node.child_by_field_name("right")?;
            continue;
        }
        let version = sources.server_version();
        expected_type(operator, dialect, version)??;
        let left_node = node.child_by_field_name("left")?;
        let right_node = node.child_by_field_name("right")?;
        let left = infer(left_node, sql, sources, Some(dialect), depth + 1);
        if let Some((pattern_node, escape_node)) = escape_operands(right_node) {
            let pattern = infer(pattern_node, sql, sources, Some(dialect), depth + 1);
            let escape = infer(escape_node, sql, sources, Some(dialect), depth + 1);
            escaped_nullable(
                (left.as_ref(), left_node),
                operator,
                (pattern.as_ref(), pattern_node),
                (escape.as_ref(), escape_node),
                sql,
                dialect,
                version,
            )?;
            return Some(PatternContinuation {
                escape: Some(false),
            });
        }
        let right = infer(right_node, sql, sources, Some(dialect), depth + 1);
        binary_nullable(
            (left.as_ref(), left_node),
            operator,
            (right.as_ref(), right_node),
            sql,
            dialect,
            version,
        )?;
        return Some(PatternContinuation {
            escape: escape_capability(&normalized_operator(operator)?, dialect, version),
        });
    }
    None
}

pub(super) fn call(
    name: &str,
    node: Node<'_>,
    sql: &str,
    sources: &ProjectionSources<'_>,
    dialect: &str,
    depth: usize,
) -> Option<ExpressionType> {
    let arity = match (dialect, name) {
        ("duckdb", "regexp_matches" | "regexp_full_match") => 2..=3,
        ("clickhouse", "match") => 2..=2,
        _ => return None,
    };
    if node.end_byte() - node.start_byte() > 64 * 1024 {
        return None;
    }
    let args = node
        .children_by_field_name("parameter", &mut node.walk())
        .collect::<Vec<_>>();
    if !arity.contains(&args.len()) || args.iter().any(|arg| arg.kind() != "term") {
        return None;
    }
    let mut nullable = false;
    for arg in args {
        let ty = infer(arg, sql, sources, Some(dialect), depth + 1);
        if !text_operand(ty.as_ref(), arg, sql, dialect) {
            return None;
        }
        nullable |= predicates::nullable(ty.as_ref(), literals::operand(arg), sql);
    }
    predicates::result(dialect, nullable)
}
