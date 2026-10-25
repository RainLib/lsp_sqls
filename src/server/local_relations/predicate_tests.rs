use super::*;

pub(super) fn empty_schema() -> Schema {
    Schema {
        id: crate::schema::SchemaId::new(),
        server: None,
        namespace: None,
        catalog: None,
        database: "public".into(),
        server_version: None,
        functions: vec![],
        source_uri: None,
        tables: vec![],
    }
}

pub(super) fn output(dialect: &str, expression: &str) -> Column {
    let columns = infer_query_output_columns(
        &format!("SELECT {expression} AS value"),
        &empty_schema(),
        "oxide://predicates.sql",
        1,
        Some(dialect),
    );
    assert_eq!(columns.len(), 1, "{dialect}: {expression}: {columns:?}");
    columns.into_iter().next().unwrap()
}

#[test]
fn fixed_predicate_types_preserve_three_valued_nullability() {
    for (dialect, ty) in [
        ("postgres", "boolean"),
        ("mysql", "bigint"),
        ("sqlite", "integer"),
    ] {
        for (expression, nullable) in [
            ("EXISTS(SELECT NULL)", false),
            ("NOT EXISTS(SELECT NULL WHERE 1=0)", false),
            ("1 IN (1,2,3)", false),
            ("1 NOT IN (1,2,NULL)", true),
            ("NULL IN (1,2)", true),
            ("1 IN (SELECT 1)", true),
            ("NULL IS NULL", false),
            ("NULL IS NOT NULL", false),
            ("1=1", false),
            ("1=NULL", true),
            ("EXISTS(SELECT 1) AND EXISTS(SELECT 2)", false),
            ("EXISTS(SELECT 1) OR NULL", true),
        ] {
            let column = output(dialect, expression);
            assert_eq!(column.data_type, ty, "{dialect}: {expression}");
            assert_eq!(column.nullable, nullable, "{dialect}: {expression}");
        }
    }
}

#[test]
fn analytic_profiles_keep_exists_and_nested_predicates_as_values() {
    for (dialect, ty) in [
        ("duckdb", "boolean"),
        ("hive", "boolean"),
        ("clickhouse", "UInt8"),
    ] {
        for expression in [
            "EXISTS(SELECT NULL, 'opaque AND OR')",
            "NOT EXISTS(SELECT NULL WHERE 1=0)",
            "NOT (NOT EXISTS(SELECT NULL))",
            "EXISTS(SELECT 1) AND NOT EXISTS(SELECT 1 WHERE 1=0)",
            "CASE WHEN 1=1 THEN EXISTS(SELECT NULL) ELSE NOT EXISTS(SELECT NULL WHERE 1=0) END",
        ] {
            let column = output(dialect, expression);
            assert_eq!(column.data_type, ty, "{dialect}: {expression}");
            assert!(!column.nullable, "{dialect}: {expression}");
        }
        let column = output(dialect, "CASE WHEN 1=1 THEN EXISTS(SELECT 1) ELSE NULL END");
        assert_eq!(column.data_type, ty, "{dialect}");
        assert!(column.nullable, "{dialect}");
    }
}

#[test]
fn analytic_predicates_preserve_engine_specific_null_and_truth_rules() {
    for dialect in ["duckdb", "hive"] {
        for (expression, nullable) in [
            ("1=1", false),
            ("1=NULL", true),
            ("NULL IS NULL", false),
            ("1 IN (1,2)", false),
            ("1 NOT IN (2,NULL)", true),
            ("NULL BETWEEN 1 AND 2", true),
        ] {
            let column = output(dialect, expression);
            assert_eq!(column.data_type, "boolean", "{dialect}: {expression}");
            assert_eq!(column.nullable, nullable, "{dialect}: {expression}");
        }
    }
    for (expression, nullable) in [
        ("1=1", false),
        ("1=NULL", true),
        ("NULL IS NULL", false),
        ("1 IN (1,NULL)", false),
        ("NULL NOT IN (1,NULL)", false),
        ("1 IN (SELECT NULL)", false),
    ] {
        let column = output("clickhouse", expression);
        assert_eq!(column.data_type, "UInt8", "{expression}");
        assert_eq!(column.nullable, nullable, "{expression}");
    }
    for (dialect, expression) in [
        ("clickhouse", "1 IS TRUE"),
        ("hive", "TRUE IS UNKNOWN"),
        ("duckdb", "CAST('2026-01-01' AS DATE) AND TRUE"),
        ("clickhouse", "CAST(1 AS Decimal64(2)) AND 1"),
        ("hive", "1 AND TRUE"),
    ] {
        assert_eq!(
            output(dialect, expression).data_type,
            "unknown",
            "{dialect}: {expression}"
        );
    }
}

#[test]
fn null_safe_operators_and_comment_separated_keywords_keep_nonnull_result() {
    for (dialect, expression, ty) in [
        ("postgres", "NULL IS DISTINCT FROM NULL", "boolean"),
        (
            "postgres",
            "1 IS /* FROM */ NOT DISTINCT /* 😀 */ FROM NULL",
            "boolean",
        ),
        ("postgres", "(NULL::boolean) IS NOT TRUE", "boolean"),
        ("mysql", "NULL <=> NULL", "bigint"),
        ("sqlite", "NULL IS 1", "integer"),
    ] {
        let column = output(dialect, expression);
        assert_eq!(column.data_type, ty, "{expression}");
        assert!(!column.nullable, "{expression}");
    }
}

#[test]
fn pattern_and_range_predicates_compose_without_losing_type() {
    for expression in [
        "1 BETWEEN 0 AND 2",
        "1 NOT BETWEEN 0 AND 2",
        "'oxide' LIKE 'ox%'",
        "'oxide' NOT ILIKE 'db%'",
        "'oxide' SIMILAR TO '(ox|db)%'",
        "NOT ('oxide' ILIKE 'ox%' AND 2 BETWEEN 1 AND 3)",
    ] {
        let column = output("postgres", expression);
        assert_eq!(column.data_type, "boolean", "{expression}");
        assert!(!column.nullable, "{expression}");
    }
    assert!(output("postgres", "1 BETWEEN NULL AND 2").nullable);
}

#[test]
fn integer_case_and_set_null_branches_keep_dialect_identity() {
    for (dialect, ty) in [
        ("mysql", "bigint"),
        ("sqlserver", "int"),
        ("tsql", "int"),
        ("mssql", "int"),
    ] {
        for (expression, nullable) in [
            ("CASE WHEN EXISTS(SELECT 1) THEN 1 ELSE 0 END", false),
            ("CASE WHEN EXISTS(SELECT 1) THEN 1 ELSE NULL END", true),
            ("CASE WHEN EXISTS(SELECT 1) THEN NULL ELSE 1 END", true),
            ("CASE WHEN EXISTS(SELECT 1) THEN 1 END", true),
        ] {
            let column = output(dialect, expression);
            assert_eq!(column.data_type, ty, "{dialect}: {expression}");
            assert_eq!(column.nullable, nullable, "{dialect}: {expression}");
        }
        let columns = infer_query_output_columns(
            "SELECT NULL AS value UNION ALL SELECT 1",
            &empty_schema(),
            "oxide://predicates.sql",
            1,
            Some(dialect),
        );
        assert_eq!(columns[0].data_type, ty);
        assert!(columns[0].nullable);
    }
    assert_eq!(
        output(
            "mysql",
            "CASE WHEN 1=1 THEN CAST(1 AS DECIMAL) ELSE CAST(2 AS DECIMAL) END"
        )
        .data_type,
        "unknown"
    );
}

#[test]
fn predicate_subqueries_do_not_spend_scalar_output_branch_budget() {
    let branches = (0..100)
        .map(|_| "SELECT 1")
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    for expression in [format!("EXISTS({branches})"), format!("1 IN ({branches})")] {
        assert_eq!(output("postgres", &expression).data_type, "boolean");
        let columns = infer_query_output_columns(
            &format!("SELECT {expression} AS predicate, (SELECT 1) AS scalar"),
            &empty_schema(),
            "oxide://predicates.sql",
            1,
            Some("postgres"),
        );
        assert_eq!(columns.len(), 2);
        assert_eq!(
            columns[1].data_type, "integer",
            "predicate body must not starve its scalar neighbor"
        );
    }
    assert_eq!(
        output("postgres", "(SELECT 1) IN (1,2)").data_type,
        "boolean"
    );
    let list = (0..300).map(|_| "1").collect::<Vec<_>>().join(",");
    let column = output("postgres", &format!("1 IN ({list})"));
    assert_eq!(column.data_type, "boolean");
    assert!(
        column.nullable,
        "bounded list analysis remains conservative"
    );
}

#[test]
fn unsupported_dialects_and_operand_types_remain_unknown() {
    for (dialect, expression) in [
        ("sqlserver", "EXISTS(SELECT 1)"),
        ("sqlserver", "1=1"),
        ("postgres", "mystery()=mystery()"),
        (
            "postgres",
            "CAST(NULL AS custom_type)=CAST(NULL AS custom_type)",
        ),
        ("postgres", "CAST(NULL AS json)=CAST(NULL AS json)"),
        ("sqlite", "CASE WHEN 1=1 THEN 1 ELSE 0 END"),
    ] {
        assert_eq!(
            output(dialect, expression).data_type,
            "unknown",
            "{dialect}: {expression}"
        );
    }
}
