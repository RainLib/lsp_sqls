//! Engine-specific numeric rules; neither SQL modes nor UDF overloads are guessed.
use super::{numeric_base, ExpressionType};

pub(super) fn binary(
    left: ExpressionType,
    op: &str,
    right: ExpressionType,
    dialect: &str,
) -> Option<ExpressionType> {
    if !matches!(op, "+" | "-" | "*" | "/" | "%") {
        return None;
    }
    let ty = match dialect {
        "postgres" => {
            let left_rank = pg_rank(&left.data_type).or_else(|| {
                (left.data_type == "null")
                    .then(|| pg_rank(&right.data_type))
                    .flatten()
            })?;
            let right_rank = pg_rank(&right.data_type).or_else(|| {
                (right.data_type == "null")
                    .then(|| pg_rank(&left.data_type))
                    .flatten()
            })?;
            let rank = left_rank.max(right_rank);
            if op == "%" && rank > 3 {
                return None;
            }
            [
                "smallint",
                "integer",
                "bigint",
                "numeric",
                "real",
                "double precision",
            ][rank]
                .to_string()
        }
        "mysql" => {
            if op == "%" {
                return None;
            }
            let (left_rank, left_unsigned) = mysql_kind(&left.data_type)?;
            let (right_rank, right_unsigned) = mysql_kind(&right.data_type)?;
            let rank = left_rank.max(right_rank);
            if rank == 2 {
                "double".into()
            } else if rank == 1 || op == "/" {
                "decimal".into()
            } else if op == "-" && (left_unsigned || right_unsigned) {
                return None;
            } else if left_unsigned || right_unsigned {
                "bigint unsigned".into()
            } else {
                "bigint".into()
            }
        }
        "sqlserver" | "mssql" | "tsql" => {
            let rank = tsql_rank(&left.data_type)?.max(tsql_rank(&right.data_type)?);
            if op == "%" && rank > 4 {
                return None;
            }
            [
                "tinyint", "smallint", "int", "bigint", "decimal", "real", "float",
            ][rank]
                .into()
        }
        _ => return None,
    };
    Some(ExpressionType {
        data_type: ty,
        nullable: left.nullable
            || right.nullable
            || (dialect == "mysql" && op == "/")
            || matches!(dialect, "sqlserver" | "mssql" | "tsql"),
    })
}

pub(super) fn unary(op: &str, argument: ExpressionType, dialect: &str) -> Option<ExpressionType> {
    if dialect != "postgres" || !matches!(op, "+" | "-") {
        return None;
    }
    let rank = pg_rank(&argument.data_type)?;
    Some(ExpressionType {
        data_type: [
            "smallint",
            "integer",
            "bigint",
            "numeric",
            "real",
            "double precision",
        ][rank]
            .into(),
        nullable: argument.nullable,
    })
}

pub(super) fn pg_rank(ty: &str) -> Option<usize> {
    let ty = crate::postgres_types::builtin_type(ty)?;
    if ty.array {
        return None;
    }
    match ty.name {
        "smallint" | "int2" => Some(0),
        "integer" | "int" | "int4" => Some(1),
        "bigint" | "int8" => Some(2),
        "numeric" | "decimal" => Some(3),
        "real" | "float4" => Some(4),
        "double precision" | "float8" => Some(5),
        _ => None,
    }
}

fn mysql_kind(ty: &str) -> Option<(u8, bool)> {
    let ty = ty.to_ascii_lowercase();
    let base = ty.split(['(', ' ']).next()?;
    let unsigned = ty
        .split_whitespace()
        .any(|word| matches!(word, "unsigned" | "zerofill"))
        || base == "unsigned";
    let rank = match base {
        "tinyint" | "smallint" | "mediumint" | "integer" | "int" | "bigint" | "signed"
        | "unsigned" => 0,
        "numeric" | "decimal" => 1,
        "real" | "float" | "double" => 2,
        _ => return None,
    };
    Some((rank, unsigned))
}

fn tsql_rank(ty: &str) -> Option<usize> {
    match numeric_base(ty).to_ascii_lowercase().as_str() {
        "tinyint" => Some(0),
        "smallint" => Some(1),
        "int" | "integer" => Some(2),
        "bigint" => Some(3),
        "decimal" | "numeric" => Some(4),
        "real" => Some(5),
        "float" => Some(
            if ty
                .split_once('(')
                .and_then(|(_, tail)| tail.trim_end_matches(')').trim().parse::<u8>().ok())
                .is_some_and(|precision| precision <= 24)
            {
                5
            } else {
                6
            },
        ),
        _ => None,
    }
}

pub(super) fn numeric_literal(text: &str, dialect: &str) -> Option<ExpressionType> {
    if !matches!(
        dialect,
        "postgres" | "mysql" | "sqlserver" | "mssql" | "tsql"
    ) {
        return None;
    }
    if text.eq_ignore_ascii_case("NULL") {
        return Some(ExpressionType::new("null", true));
    }
    if dialect == "postgres"
        && (text.eq_ignore_ascii_case("TRUE") || text.eq_ignore_ascii_case("FALSE"))
    {
        return Some(ExpressionType::new("boolean", false));
    }
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let mut parts = unsigned.split(['e', 'E']);
    let mantissa = parts.next()?;
    let exponent = parts.next();
    if parts.next().is_some() {
        return None;
    }
    if let Some(exponent) = exponent {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if exponent.is_empty() || !exponent.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
    }
    if !mantissa
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
        || !mantissa.bytes().any(|byte| byte.is_ascii_digit())
        || mantissa.bytes().filter(|byte| *byte == b'.').count() > 1
    {
        return None;
    }
    if matches!(dialect, "sqlserver" | "mssql" | "tsql") {
        let ty = if exponent.is_some() {
            "float"
        } else if mantissa.contains('.')
            || unsigned
                .parse::<u64>()
                .map_or(true, |value| value > i32::MAX as u64)
        {
            "decimal"
        } else {
            "int"
        };
        return Some(ExpressionType::new(ty, false));
    }
    let ty = if exponent.is_some() {
        if dialect == "mysql" {
            "double"
        } else {
            "numeric"
        }
    } else if mantissa.contains('.') {
        if dialect == "mysql" {
            "decimal"
        } else {
            "numeric"
        }
    } else if dialect == "postgres" && text.parse::<i32>().is_ok() {
        "integer"
    } else if text.parse::<i64>().is_ok() {
        "bigint"
    } else if dialect == "mysql" && text.parse::<u64>().is_ok() {
        "bigint unsigned"
    } else if dialect == "mysql" {
        "decimal"
    } else {
        "numeric"
    };
    Some(ExpressionType::new(ty, false))
}
