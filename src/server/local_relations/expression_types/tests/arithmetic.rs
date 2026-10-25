use super::{check, output};

#[test]
fn postgres_numeric_operators_promote_widths_and_remove_typmods() {
    for (expr, ty) in [
        ("i.small + i.small", "smallint"),
        ("i.small + i.amount", "integer"),
        ("i.amount * i.large", "bigint"),
        ("i.precise * i.amount", "numeric"),
        ("i.fraction + i.precise", "real"),
        (
            "CAST(i.amount AS double precision) / i.large",
            "double precision",
        ),
        ("i.large % i.amount", "bigint"),
        ("i.precise % i.amount", "numeric"),
    ] {
        check("postgres", expr, ty, true);
    }
}

#[test]
fn postgres_nested_arithmetic_composes_with_aggregates_case_and_coalesce() {
    for (expr, ty, nullable) in [
        ("1 + 2 * 3", "integer", false),
        ("-(1 + 2)", "integer", false),
        ("SUM(i.amount * i.amount)", "bigint", true),
        ("SUM(i.large * i.amount)", "numeric", true),
        ("COALESCE(SUM(i.large * i.amount), 0)", "numeric", false),
        (
            "CASE WHEN i.amount > 0 THEN i.amount * i.large ELSE 0 END",
            "bigint",
            true,
        ),
        (
            "AVG(i.amount + i.small) OVER (PARTITION BY i.label)",
            "numeric",
            true,
        ),
    ] {
        check("postgres", expr, ty, nullable);
    }
}

#[test]
fn mysql_arithmetic_is_not_postgres_integer_arithmetic() {
    for (expr, ty) in [
        ("i.small + i.amount", "bigint"),
        ("i.amount * i.large", "bigint"),
        ("i.amount / i.large", "decimal"),
        ("i.precise * i.amount", "decimal"),
        ("i.fraction + i.large", "double"),
        ("i.unsigned_count + i.amount", "bigint unsigned"),
        ("SUM(i.amount * i.large)", "decimal"),
    ] {
        check("mysql", expr, ty, true);
    }
}

#[test]
fn unsigned_subtraction_and_overloads_are_not_guessed() {
    for (dialect, expr) in [
        ("mysql", "i.unsigned_count - i.amount"),
        ("postgres", "i.label + i.amount"),
        ("postgres", "i.fraction % i.amount"),
        ("postgres", "mystery(i.amount) + i.large"),
        ("sqlite", "i.amount + i.large"),
        ("hive", "i.amount * i.large"),
        ("clickhouse", "i.amount / i.large"),
    ] {
        check(dialect, expr, "unknown", true);
    }
}

#[test]
fn postgres_typed_nulls_and_unary_operators_keep_types() {
    for (expr, ty, nullable) in [
        ("-(i.amount)", "integer", true),
        ("+(i.precise)", "numeric", true),
        ("NULL::bigint + 1", "bigint", true),
        ("-CAST(1 AS smallint)", "smallint", false),
        ("1 / 0", "integer", false),
    ] {
        check("postgres", expr, ty, nullable);
    }
}

#[test]
fn try_cast_keeps_target_type_and_failure_nullability() {
    for expr in [
        "TRY_CAST(i.label AS bigint)",
        "TRY_CAST(i.amount AS decimal(18, 4))",
        "TRY_CAST(CAST(i.label AS varchar(20)) AS int)",
    ] {
        let expected = if expr.contains("decimal") {
            "decimal(18, 4)"
        } else if expr.ends_with("AS int)") {
            "int"
        } else {
            "bigint"
        };
        let column = output("tsql", expr);
        if column.data_type != expected {
            let mut parser = crate::parser::SqlParser::new();
            eprintln!(
                "{}",
                parser
                    .parse_syntax_tree(&format!("SELECT {expr} AS result FROM items i"))
                    .unwrap()
                    .root_node()
                    .to_sexp()
            );
        }
        assert_eq!(
            (column.data_type.as_str(), column.nullable),
            (expected, true),
            "{expr}"
        );
    }
}

#[test]
fn mysql_literals_and_division_keep_engine_numeric_families() {
    for (expr, ty, nullable) in [
        ("1 + 2", "bigint", false),
        ("1.2 * i.amount", "decimal", true),
        ("1e3 + i.large", "double", true),
        ("1 / 0", "decimal", true),
        ("SUM(i.amount * 2)", "decimal", true),
        ("18446744073709551615 + i.amount", "bigint unsigned", true),
    ] {
        check("mysql", expr, ty, nullable);
    }
}

#[test]
fn tsql_numeric_precedence_composes_with_try_cast_and_large_constants() {
    for (expr, ty) in [
        ("i.small + i.amount", "int"),
        ("i.amount * i.large", "bigint"),
        ("TRY_CAST(i.label AS decimal(18,4)) + 1", "decimal"),
        ("2147483648 / 2", "decimal"),
        ("CAST(i.amount AS float(24)) + i.large", "real"),
        ("CAST(i.amount AS float(53)) / i.large", "float"),
    ] {
        check("tsql", expr, ty, true);
    }
    for dialect in ["postgres", "mysql"] {
        check(dialect, "TRY_CAST(i.label AS bigint)", "unknown", true);
    }
}
