//! Set-output joins retain engine-specific rules. Identity of fixed integral
//! types is safe without guessing numeric precision, collation or UDT identity.
use super::{common_postgres_type, ExpressionType};

pub(in crate::server::local_relations) fn merge(
    dialect: Option<&str>,
    left: ExpressionType,
    right: ExpressionType,
) -> Option<ExpressionType> {
    if left.data_type == "unknown" || right.data_type == "unknown" {
        return None;
    }
    if dialect == Some("hive") {
        let null = left.data_type == "null" || right.data_type == "null";
        if left.data_type != "null"
            && right.data_type != "null"
            && !left.data_type.eq_ignore_ascii_case(&right.data_type)
        {
            return None;
        }
        return Some(ExpressionType {
            data_type: if left.data_type == "null" {
                right.data_type
            } else {
                left.data_type
            },
            nullable: left.nullable || right.nullable || null,
        });
    }
    if dialect == Some("postgres") {
        if left.data_type == "null" && right.data_type == "null" {
            return Some(ExpressionType::new("text", true));
        }
        return common_postgres_type(&[left, right]);
    }
    let left_null = left.data_type == "null";
    let right_null = right.data_type == "null";
    if matches!(dialect, Some("sqlserver" | "mssql" | "tsql")) {
        let left_type = (!left_null)
            .then(|| crate::server::tsql_types::scalar_type(&left.data_type))
            .flatten();
        let right_type = (!right_null)
            .then(|| crate::server::tsql_types::scalar_type(&right.data_type))
            .flatten();
        let ty = if left_null {
            right_type?
        } else if right_null {
            left_type?
        } else {
            let left = left_type?;
            let right = right_type?;
            if left == right {
                left
            } else {
                let rank = |ty: &str| match ty {
                    "bit" => Some(0),
                    "tinyint" => Some(1),
                    "smallint" => Some(2),
                    "int" | "integer" => Some(3),
                    "bigint" => Some(4),
                    _ => None,
                };
                ["bit", "tinyint", "smallint", "int", "bigint"][rank(&left)?.max(rank(&right)?)]
                    .into()
            }
        };
        return Some(ExpressionType {
            data_type: ty,
            nullable: left.nullable || right.nullable || left_null || right_null,
        });
    }
    if !left_null && !right_null && !left.data_type.eq_ignore_ascii_case(&right.data_type) {
        return None;
    }
    let data_type = if left_null {
        right.data_type
    } else {
        left.data_type
    };
    let name = data_type.to_ascii_lowercase();
    // Preserve identical modeled logical scalar types through CASE/set
    // branches without inventing these engines' wider implicit-cast lattice.
    if matches!(dialect, Some("duckdb" | "clickhouse" | "sqlite"))
        && crate::dialects::common::type_supports_logical_continuation(&data_type, dialect.unwrap())
            == Some(true)
    {
        return Some(ExpressionType {
            data_type,
            nullable: left.nullable || right.nullable || left_null || right_null,
        });
    }
    let integral = match dialect {
        Some("mysql" | "mariadb") => matches!(
            name.as_str(),
            "tinyint"
                | "smallint"
                | "mediumint"
                | "int"
                | "integer"
                | "bigint"
                | "tinyint unsigned"
                | "smallint unsigned"
                | "mediumint unsigned"
                | "int unsigned"
                | "integer unsigned"
                | "bigint unsigned"
        ),
        Some("sqlserver" | "mssql" | "tsql") => matches!(
            name.as_str(),
            "bit" | "tinyint" | "smallint" | "int" | "integer" | "bigint"
        ),
        _ => false,
    };
    integral.then_some(ExpressionType {
        data_type,
        nullable: left.nullable || right.nullable,
    })
}
