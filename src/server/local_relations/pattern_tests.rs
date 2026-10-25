use super::predicate_tests::output;

#[test]
fn clickhouse_negative_regex_aliases_require_the_introducing_version() {
    for (version, expression, expected) in [
        (None, "'x' !~ 'X'", "unknown"),
        (Some("26.7"), "'x' !~ 'X'", "unknown"),
        (Some("26.8"), "'x' !~ 'X'", "UInt8"),
        (Some("26.8"), "'x' !~* 'X'", "UInt8"),
        (Some("26.8"), "'x' NOT REGEXP 'X'", "unknown"),
    ] {
        let mut schema = super::predicate_tests::empty_schema();
        schema.server_version = version.map(str::to_owned);
        let columns = super::infer_query_output_columns(
            &format!("SELECT {expression} AS flag"),
            &schema,
            "oxide://negative-pattern.sql",
            1,
            Some("clickhouse"),
        );
        assert_eq!(columns[0].data_type, expected, "{version:?}: {expression}");
    }
}

#[test]
fn pattern_results_keep_product_types_through_operators_calls_and_conditions() {
    for (profile, expression, expected) in [
        ("duckdb", "'oxide' ILIKE 'ox%'", "boolean"),
        ("duckdb", "'oxide' SIMILAR TO '(ox|db).*'", "boolean"),
        ("duckdb", "regexp_matches('oxide','^ox')", "boolean"),
        ("duckdb", "regexp_full_match('oxide','ox.*','c')", "boolean"),
        ("duckdb", "'oxide.txt' GLOB '*.txt'", "boolean"),
        ("clickhouse", "'oxide' ILIKE 'ox%'", "UInt8"),
        ("clickhouse", "match('oxide','^ox')", "UInt8"),
        ("clickhouse", "'oxide' REGEXP '^ox'", "UInt8"),
        ("hive", "'oxide' RLIKE '^ox'", "boolean"),
        ("hive", "'oxide' NOT /*😀*/ REGEXP '^db'", "boolean"),
        ("mysql", "'oxide' REGEXP '^ox'", "bigint"),
        ("mysql", "'oxide' NOT RLIKE '^db'", "bigint"),
        ("sqlite", "'oxide' GLOB 'ox*'", "integer"),
        ("sqlite", "'oxide' NOT GLOB 'db*'", "integer"),
        ("postgres", "'oxide' ~* '^OX'", "boolean"),
    ] {
        let column = output(profile, expression);
        assert_eq!(column.data_type, expected, "{profile}: {expression}");
        let nested = output(
            profile,
            &format!("CASE WHEN 1=1 THEN {expression} ELSE NULL END"),
        );
        assert_eq!(nested.data_type, expected, "CASE {profile}: {expression}");
        assert!(nested.nullable);
    }
}

#[test]
fn pattern_nullability_tracks_null_arguments_without_compiling_regular_expressions() {
    for (profile, expression, expected, nullable) in [
        ("duckdb", "regexp_matches('oxide','^ox')", "boolean", false),
        ("duckdb", "regexp_matches(NULL,'^ox')", "boolean", true),
        ("duckdb", "NULL ILIKE 'x%'", "boolean", true),
        ("clickhouse", "match('oxide','^ox')", "UInt8", false),
        (
            "clickhouse",
            "match(CAST(NULL AS Nullable(String)),'^ox')",
            "UInt8",
            true,
        ),
        ("hive", "NULL RLIKE '^ox'", "boolean", true),
        ("mysql", "NULL REGEXP '^ox'", "bigint", true),
        ("sqlite", "NULL GLOB 'ox*'", "integer", true),
    ] {
        let column = output(profile, expression);
        assert_eq!(column.data_type, expected, "{profile}: {expression}");
        assert_eq!(column.nullable, nullable, "{profile}: {expression}");
    }
}

#[test]
fn pattern_fallback_does_not_invent_extensions_namespaces_arity_or_scalar_support() {
    for (profile, expression) in [
        ("sqlite", "'oxide' REGEXP '^ox'"),
        ("sqlserver", "'oxide' ILIKE 'ox%'"),
        ("hive", "'oxide' ILIKE 'ox%'"),
        ("clickhouse", "'oxide' SIMILAR TO 'ox.*'"),
        ("duckdb", "private.regexp_matches('oxide','^ox')"),
        ("clickhouse", "private.match('oxide','^ox')"),
        ("duckdb", "regexp_matches('oxide')"),
        ("duckdb", "regexp_matches('oxide','^ox','c','extra')"),
        ("clickhouse", "match('oxide','^ox','c')"),
        ("clickhouse", "MATCH('oxide','^ox')"),
        ("duckdb", "regexp_matches('oxide','^ox', unknown => 'c')"),
        ("duckdb", "'oxide' NOT GLOB 'db*'"),
        ("duckdb", "regexp_matches(CAST(1 AS INTEGER),'x')"),
    ] {
        assert_eq!(
            output(profile, expression).data_type,
            "unknown",
            "{profile}: {expression}"
        );
    }
}

#[test]
fn pattern_version_gates_do_not_borrow_modern_operator_support() {
    for (profile, version, expression, expected) in [
        ("duckdb", "1.5.4", "'x' ~* 'X'", "unknown"),
        ("duckdb", "2.0", "'x' ~* 'X'", "boolean"),
        ("clickhouse", "26.7", "'x' ~* 'X'", "unknown"),
        ("clickhouse", "26.8", "'x' ~* 'X'", "UInt8"),
        ("clickhouse", "20.5", "'x' ILIKE 'X'", "unknown"),
        ("clickhouse", "20.6", "'x' ILIKE 'X'", "UInt8"),
    ] {
        let mut schema = super::predicate_tests::empty_schema();
        schema.server_version = Some(version.into());
        let sql = format!("SELECT {expression} AS flag");
        let columns = super::infer_query_output_columns(
            &sql,
            &schema,
            "oxide://pattern-version.sql",
            1,
            Some(profile),
        );
        assert_eq!(columns[0].data_type, expected, "{profile} {version}");
    }
}
