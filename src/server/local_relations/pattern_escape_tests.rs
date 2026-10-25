use super::predicate_tests::output;

#[test]
fn escaped_patterns_keep_product_results_through_case_and_local_projections() {
    for (profile, operator, expected) in [
        ("postgres", "LIKE", "boolean"),
        ("postgres", "NOT ILIKE", "boolean"),
        ("postgres", "SIMILAR TO", "boolean"),
        ("duckdb", "LIKE", "boolean"),
        ("duckdb", "NOT ILIKE", "boolean"),
        ("mysql", "LIKE", "bigint"),
        ("sqlite", "NOT LIKE", "integer"),
    ] {
        let expression = format!("'oxide_1' {operator} 'oxide#_1' ESCAPE '#'");
        for expression in [
            expression.clone(),
            format!("CASE WHEN 1=1 THEN {expression} ELSE NULL END"),
        ] {
            assert_eq!(
                output(profile, &expression).data_type,
                expected,
                "{profile}: {expression}"
            );
        }
    }
}

#[test]
fn escape_operands_propagate_null_and_preserve_nested_string_expressions() {
    for (profile, expression, expected, nullable) in [
        ("postgres", "'x' LIKE 'x' ESCAPE NULL", "boolean", true),
        ("duckdb", "'x' LIKE 'x' ESCAPE NULL", "boolean", true),
        (
            "duckdb",
            "'x' LIKE 'x' ESCAPE SUBSTR('##',1,1)",
            "boolean",
            true, // The unknown builtin keeps conservative nullability.
        ),
        ("sqlite", "NULL LIKE 'x' ESCAPE '#'", "integer", true),
        ("sqlite", "'x' LIKE 'x' ESCAPE 1", "integer", false),
        ("postgres", "'x' LIKE 'x' ESCAPE ''", "boolean", false),
        ("duckdb", "'x' ILIKE 'x' ESCAPE ''", "boolean", false),
    ] {
        let value = output(profile, expression);
        assert_eq!(value.data_type, expected, "{profile}: {expression}");
        assert_eq!(value.nullable, nullable, "{profile}: {expression}");
    }
}

#[test]
fn escaped_patterns_do_not_invent_other_products_operators_or_invalid_literals() {
    for (profile, expression) in [
        ("duckdb", "'x' SIMILAR TO 'x' ESCAPE '#'"),
        ("hive", "'x' LIKE 'x' ESCAPE '#'"),
        ("sqlite", "'x' GLOB 'x' ESCAPE '#'"),
        ("mysql", "'x' REGEXP 'x' ESCAPE '#'"),
        ("postgres", "'x' ~ 'x' ESCAPE '#'"),
        ("postgres", "'x' LIKE 'x' ESCAPE '##'"),
        ("sqlite", "'x' LIKE 'x' ESCAPE ''"),
        ("duckdb", "'x' LIKE 'x' ESCAPE 1"),
        ("duckdb", "'x' LIKE 'x' ESCAPE '尾'"),
        ("duckdb", "'x' LIKE 'x' ESCAPE '😀'"),
        ("sqlserver", "'x' LIKE 'x' ESCAPE '#'"),
    ] {
        assert_eq!(
            output(profile, expression).data_type,
            "unknown",
            "{profile}: {expression}"
        );
    }
}

#[test]
fn clickhouse_escape_is_bound_to_the_documented_introducing_version() {
    for (version, expected) in [
        (None, "unknown"),
        (Some("26.5.2"), "unknown"),
        (Some("26.6.1.123"), "UInt8"),
    ] {
        let mut schema = super::predicate_tests::empty_schema();
        schema.server_version = version.map(str::to_owned);
        let columns = super::infer_query_output_columns(
            "SELECT 'oxide_1' ILIKE 'oxide#_1' ESCAPE '#' AS flag",
            &schema,
            "oxide://escape-version.sql",
            1,
            Some("clickhouse"),
        );
        assert_eq!(columns[0].data_type, expected, "{version:?}");
    }
}
