mod arithmetic;
mod search;

use crate::{
    schema::{Column, Schema, SchemaId, Table},
    server::local_relations::infer_query_output_columns,
};

fn output(dialect: &str, expression: &str) -> Column {
    let schema = Schema {
        id: SchemaId::new(),
        server: None,
        namespace: None,
        catalog: None,
        database: "public".into(),
        server_version: None,
        functions: vec![],
        source_uri: None,
        tables: vec![Table {
            name: "items".into(),
            columns: [
                ("small", "smallint"),
                ("amount", "integer"),
                ("large", "bigint"),
                ("fraction", "real"),
                ("precise", "numeric(12,2)"),
                ("label", "text"),
                ("unsigned_count", "bigint unsigned"),
            ]
            .into_iter()
            .map(|(name, ty)| Column {
                name: name.into(),
                data_type: ty.into(),
                nullable: false,
                ..Default::default()
            })
            .collect(),
            ..Default::default()
        }],
    };
    let columns = infer_query_output_columns(
        &format!("SELECT {expression} AS result FROM items i"),
        &schema,
        "oxide://query/test.sql",
        1,
        Some(dialect),
    );
    assert_eq!(columns.len(), 1, "{dialect}: {expression}: {columns:?}");
    columns.into_iter().next().unwrap()
}

fn check(dialect: &str, expression: &str, ty: &str, nullable: bool) {
    let column = output(dialect, expression);
    assert_eq!(
        (column.data_type.as_str(), column.nullable),
        (ty, nullable),
        "{dialect}: {expression}"
    );
}
#[test]
fn sqlite_cast_affinity_is_dialect_specific_in_complex_projection_types() {
    for (declared, expected) in [
        ("INTEGER", "integer"),
        ("BOOLEAN", "numeric"),
        ("VARCHAR(8)", "text"),
        ("\"FLOATING POINT\"", "integer"),
        ("BLOB", "blob"),
        ("REAL", "real"),
        ("DATE", "numeric"),
    ] {
        assert_eq!(
            output("sqlite", &format!("CAST(1 AS {declared})")).data_type,
            expected
        );
    }
}

#[test]
fn fixed_scalar_results_and_nullif_share_projection_types() {
    for (expression, ty, nullable) in [
        ("LOWER(i.label)", "text", true),
        ("UPPER('x')", "text", false),
        ("LENGTH('x')", "integer", false),
        ("REPLACE('abc','a','z')", "text", false),
        ("STRPOS('abc','b')", "integer", false),
        ("NULLIF(1, 2.5)", "numeric", true),
        ("NULLIF('a','b')", "text", true),
    ] {
        check("postgres", expression, ty, nullable);
    }
}

#[test]
fn boolean_aggregates_keep_their_boolean_result_and_empty_group_nullability() {
    for expression in ["BOOL_AND(TRUE)", "BOOL_OR(TRUE)", "EVERY(TRUE)"] {
        check("postgres", expression, "boolean", true);
    }
}

#[test]
fn catalog_numeric_identities_compose_across_projection_consumers() {
    for (expression, ty, nullable) in [
        ("CAST(1 AS pg_catalog.int4) + 2", "integer", false),
        (
            "COALESCE(CAST(1 AS pg_catalog.int2), CAST(2 AS INTEGER))",
            "integer",
            false,
        ),
        (
            "CASE WHEN TRUE THEN CAST(1 AS pg_catalog.int4) ELSE CAST(2 AS INTEGER) END",
            "integer",
            false,
        ),
        ("SUM(CAST(1 AS pg_catalog.int4))", "bigint", true),
        ("MAX(CAST(1 AS pg_catalog.int4))", "pg_catalog.int4", true),
    ] {
        check("postgres", expression, ty, nullable);
    }
}

#[test]
fn catalog_boolean_and_text_identities_compose_without_custom_type_guessing() {
    for (expression, nullable) in [
        ("NOT CAST(TRUE AS pg_catalog.bool)", false),
        ("BOOL_OR(CAST(TRUE AS pg_catalog.bool))", true),
        ("CAST(TRUE AS pg_catalog.bool) AND TRUE", false),
        ("CAST('a' AS pg_catalog.text) LIKE 'a'", false),
        ("CAST(TRUE AS pg_catalog.bool) = TRUE", false),
    ] {
        check("postgres", expression, "boolean", nullable);
    }
}

#[test]
fn casts_keep_type_parameters_and_nested_expressions() {
    for expression in [
        "CAST(i.amount AS decimal(18, 4))",
        "CAST(COALESCE(i.amount, 0) AS decimal(18, 4))",
        "CAST(CASE WHEN i.amount > 0 THEN i.amount ELSE 0 END AS decimal(18, 4))",
    ] {
        check(
            "postgres",
            expression,
            "decimal(18, 4)",
            !expression.contains("COALESCE"),
        );
    }
    check("postgres", "CAST(NULL AS bigint)", "bigint", true);
    check(
        "mysql",
        "CAST(i.amount AS DECIMAL(18, 4))",
        "DECIMAL(18, 4)",
        true,
    );
    check("tsql", "CAST(i.amount AS varchar(32))", "varchar(32)", true);
}

#[test]
fn postgres_aggregates_promote_each_input_type() {
    for (expr, ty) in [
        ("SUM(i.small)", "bigint"),
        ("SUM(i.amount)", "bigint"),
        ("SUM(i.large)", "numeric"),
        ("SUM(i.precise)", "numeric"),
        ("SUM(i.fraction)", "real"),
        ("AVG(i.amount)", "numeric"),
        ("AVG(i.fraction)", "double precision"),
        ("MAX(i.label)", "text"),
    ] {
        check("postgres", expr, ty, true);
    }
}

#[test]
fn mysql_aggregates_use_decimal_or_double() {
    for (expr, ty) in [
        ("SUM(i.amount)", "decimal"),
        ("AVG(i.large)", "decimal"),
        ("SUM(i.fraction)", "double"),
        ("AVG(i.precise)", "decimal"),
    ] {
        check("mysql", expr, ty, true);
    }
}

#[test]
fn count_type_depends_on_dialect_and_sqlserver_metadata_is_nullable() {
    for (dialect, ty, nullable) in [
        ("postgres", "bigint", false),
        ("mysql", "bigint", false),
        ("sqlite", "integer", false),
        ("tsql", "int", true),
        ("sqlserver", "int", true),
    ] {
        for expr in [
            "COUNT(*)",
            "COUNT(DISTINCT i.amount)",
            "COUNT(i.amount) OVER (PARTITION BY i.label)",
        ] {
            check(dialect, expr, ty, nullable);
        }
    }
}

#[test]
fn windows_retain_function_type_and_empty_frame_nullability() {
    for expr in [
        "ROW_NUMBER() OVER (ORDER BY i.amount)",
        "RANK() OVER (PARTITION BY i.label ORDER BY i.amount)",
        "DENSE_RANK() OVER w",
    ] {
        check("postgres", expr, "bigint", false);
    }
    check(
        "postgres",
        "SUM(i.amount) OVER (ORDER BY i.amount ROWS BETWEEN 2 PRECEDING AND 1 PRECEDING)",
        "bigint",
        true,
    );
}

#[test]
fn case_uses_only_result_branches_and_missing_else_is_nullable() {
    check(
        "postgres",
        "CASE WHEN i.label = 'x' THEN i.amount ELSE i.large END",
        "bigint",
        true,
    );
    check(
        "postgres",
        "CASE i.label WHEN 'a' THEN 1 WHEN 'b' THEN 2 ELSE 3 END",
        "integer",
        false,
    );
    check(
        "postgres",
        "CASE WHEN i.amount > 0 THEN 1 END",
        "integer",
        true,
    );
    check(
        "postgres",
        "CASE WHEN i.amount > 0 THEN CAST(1 AS numeric) ELSE 2 END",
        "numeric",
        false,
    );
    check(
        "postgres",
        "CASE WHEN i.amount > 0 THEN i.amount ELSE i.label END",
        "unknown",
        true,
    );
}

#[test]
fn coalesce_combines_aggregate_promotion_with_fallback_nullability() {
    check("postgres", "COALESCE(SUM(i.amount), 0)", "bigint", false);
    check("postgres", "COALESCE(NULL, i.amount)", "integer", true);
    check(
        "postgres",
        "COALESCE(CASE WHEN i.amount > 0 THEN i.large END, CAST(0 AS bigint))",
        "bigint",
        false,
    );
    check(
        "postgres",
        "COALESCE(i.amount, app.mystery())",
        "unknown",
        true,
    );
}

#[test]
fn unsupported_dialects_udfs_ambiguity_and_dynamic_sqlite_sum_stay_unknown() {
    for (dialect, expr) in [
        ("hive", "SUM(i.amount)"),
        ("sqlite", "SUM(i.amount)"),
        ("postgres", "app.sum(i.amount)"),
        ("postgres", "\"SUM\"(i.amount)"),
        ("postgres", "mystery(i.amount)"),
        ("postgres", "SUM(i.label)"),
    ] {
        check(dialect, expr, "unknown", true);
    }
    check("postgres", "\"sum\"(i.amount)", "bigint", true);
    check("clickhouse", "COUNT(*)", "UInt64", false);
    check("clickhouse", "count()", "UInt64", false);
}

#[test]
fn nesting_budget_prevents_unbounded_expression_walk() {
    let expression = format!("{}i.amount{}", "COALESCE(".repeat(40), ", 0)".repeat(40));
    check("postgres", &expression, "unknown", true);
}
