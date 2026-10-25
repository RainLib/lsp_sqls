use super::*;
use crate::{position::lsp_position_at_end, schema::SchemaId};

fn catalog() -> Schema {
    Schema {
        id: SchemaId::new(),
        server: None,
        namespace: None,
        catalog: None,
        database: "public".into(),
        server_version: None,
        functions: vec![],
        source_uri: None,
        tables: vec![Table {
            name: "users".into(),
            columns: [("id", "integer"), ("large", "bigint"), ("name", "text")]
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
    }
}

fn relation(dialect: &str, body: &str, name: &str) -> Vec<Column> {
    let sql = format!("{body} SELECT {name}. FROM {name}");
    let schema = augment_schema_with_local_relations(
        catalog(),
        &sql,
        lsp_position_at_end(&sql),
        "oxide://query/position.sql",
        Some(dialect),
    );
    schema
        .tables
        .into_iter()
        .find(|table| table.name == name)
        .unwrap_or_else(|| panic!("missing {name}: {body}"))
        .columns
}

fn types(columns: &[Column]) -> Vec<(&str, &str)> {
    columns
        .iter()
        .map(|column| (column.name.as_str(), column.data_type.as_str()))
        .collect()
}

#[test]
fn explicit_cte_names_keep_unnamed_expression_positions_and_types() {
    let columns = relation("postgres", "WITH totals(n, ident, fallback) AS (SELECT COUNT(*), u.id, COALESCE(SUM(u.large), 0) FROM users u GROUP BY u.id)", "totals");
    assert_eq!(
        types(&columns),
        [
            ("n", "bigint"),
            ("ident", "integer"),
            ("fallback", "numeric")
        ]
    );
}

#[test]
fn cte_names_rename_stars_without_losing_remaining_projection_types() {
    let columns = relation(
        "postgres",
        "WITH renamed(first_id) AS (SELECT u.*, COUNT(*) OVER () AS n FROM users u)",
        "renamed",
    );
    assert_eq!(
        types(&columns),
        [
            ("first_id", "integer"),
            ("large", "bigint"),
            ("name", "text"),
            ("n", "bigint")
        ]
    );
}

#[test]
fn derived_correlation_list_names_unnamed_casts_by_position() {
    let sql = "SELECT d. FROM (SELECT COUNT(*) OVER (), CAST(u.id AS numeric(18,2)), u.name FROM users u) d(n, amount, label)";
    let schema = augment_schema_with_local_relations(
        catalog(),
        sql,
        lsp_position_at_end("SELECT d."),
        "oxide://query/derived.sql",
        Some("postgres"),
    );
    let columns = &schema
        .tables
        .iter()
        .find(|table| table.name == "d")
        .unwrap()
        .columns;
    assert_eq!(
        types(columns),
        [
            ("n", "bigint"),
            ("amount", "numeric(18,2)"),
            ("label", "text")
        ]
    );
}

#[test]
fn postgres_postfix_casts_keep_nested_and_array_target_types() {
    for (expr, expected) in [
        ("u.id::bigint", "bigint"),
        ("(COALESCE(u.id, 0))::numeric(18,2)", "numeric(18,2)"),
        ("u.id::text::varchar(32)", "varchar(32)"),
        ("NULL::integer[]", "integer[]"),
    ] {
        let columns = infer_query_output_columns(
            &format!("SELECT {expr} AS result FROM users u"),
            &catalog(),
            "oxide://query/postgres.sql",
            1,
            Some("postgres"),
        );
        if types(&columns) != [("result", expected)] {
            let mut parser = SqlParser::new();
            let tree = parser
                .parse_syntax_tree(&format!("SELECT {expr} AS result FROM users u"))
                .unwrap();
            eprintln!("AST {}", tree.root_node().to_sexp());
        }
        assert_eq!(types(&columns), [("result", expected)], "{expr}");
    }
}

#[test]
fn postgres_union_merges_positional_types_and_keeps_left_output_names() {
    let columns = relation("postgres", "WITH combined AS (SELECT u.id AS key, COUNT(*) AS n FROM users u GROUP BY u.id UNION ALL SELECT u.large AS other, SUM(u.large) AS total FROM users u GROUP BY u.large)", "combined");
    assert_eq!(types(&columns), [("key", "bigint"), ("n", "numeric")]);
}

#[test]
fn null_union_resolution_respects_pairwise_grouping() {
    let columns = relation(
        "postgres",
        "WITH combined(value) AS (SELECT NULL UNION SELECT 1)",
        "combined",
    );
    assert_eq!(types(&columns), [("value", "integer")]);
    let columns = relation(
        "postgres",
        "WITH combined(value) AS (SELECT NULL UNION SELECT NULL UNION SELECT 1)",
        "combined",
    );
    assert_eq!(types(&columns), [("value", "unknown")]);
    let columns = relation(
        "postgres",
        "WITH combined(value) AS (SELECT NULL UNION (SELECT NULL UNION SELECT 1))",
        "combined",
    );
    assert_eq!(types(&columns), [("value", "integer")]);
}

#[test]
fn intersect_precedence_and_parentheses_control_type_resolution() {
    for (body, expected) in [
        (
            "SELECT NULL UNION SELECT NULL INTERSECT SELECT 1",
            "integer",
        ),
        (
            "SELECT NULL EXCEPT SELECT NULL INTERSECT SELECT 1",
            "integer",
        ),
        (
            "(SELECT NULL UNION SELECT NULL) INTERSECT SELECT 1",
            "unknown",
        ),
        ("SELECT NULL UNION SELECT NULL EXCEPT SELECT 1", "unknown"),
    ] {
        let columns = relation(
            "postgres",
            &format!("WITH merged(value) AS ({body})"),
            "merged",
        );
        assert_eq!(types(&columns), [("value", expected)], "{body}");
    }
}

#[test]
fn union_stars_bind_each_branch_and_widen_numeric_families() {
    for (left, right, expected) in [
        ("integer", "bigint", "bigint"),
        ("numeric(12,2)", "numeric(18,4)", "numeric"),
        ("integer", "real", "real"),
        ("numeric", "double precision", "double precision"),
    ] {
        let columns = relation("postgres", &format!("WITH merged(value) AS (SELECT a.* FROM (SELECT CAST(1 AS {left}) AS first_name) a UNION SELECT b.* FROM (SELECT CAST(2 AS {right}) AS other_name) b)"), "merged");
        assert_eq!(types(&columns), [("value", expected)], "{left}/{right}");
    }
}

#[test]
fn unsupported_set_types_arity_and_unresolved_stars_do_not_shift_names() {
    for body in [
        "SELECT u.id UNION SELECT u.name FROM users u",
        "SELECT u.id, u.name FROM users u UNION SELECT u.large FROM users u",
        "SELECT mystery() AS x UNION SELECT 1",
        "SELECT missing.*, 1 AS x FROM missing",
    ] {
        let columns = relation(
            "postgres",
            &format!("WITH merged(first, second) AS ({body})"),
            "merged",
        );
        assert_eq!(
            types(&columns),
            [("first", "unknown"), ("second", "unknown")],
            "{body}"
        );
    }
    let columns = relation(
        "mysql",
        "WITH merged(value) AS (SELECT CAST(1 AS DECIMAL) UNION SELECT CAST(2 AS DECIMAL))",
        "merged",
    );
    assert_eq!(types(&columns), [("value", "unknown")]);
}

#[test]
fn explicit_names_and_types_flow_through_multiple_ctes_and_dialects() {
    for dialect in ["postgres", "mysql", "sqlite", "hive", "clickhouse"] {
        let columns = relation(dialect, "WITH first(renamed_id, renamed_name) AS (SELECT u.id, u.name FROM users u), second(final_id, final_name) AS (SELECT f.* FROM first f)", "second");
        assert_eq!(
            types(&columns),
            [("final_id", "integer"), ("final_name", "text")],
            "{dialect}"
        );
    }
}

#[test]
fn comments_unicode_and_nullability_survive_positional_projection() {
    let columns = relation("postgres", "WITH \"输出\"(\"编号\", \"合计\") AS (SELECT /* 🧭 */ u.id /* first */, COALESCE(SUM(u.large), 0) /* second */ FROM users u GROUP BY u.id)", "输出");
    assert_eq!(types(&columns), [("编号", "integer"), ("合计", "numeric")]);
    assert!(!columns[1].nullable);
    let columns = relation(
        "postgres",
        "WITH renamed(a,b,c) AS (SELECT \"客户\" . * /* trailing comment */ FROM users \"客户\")",
        "renamed",
    );
    assert_eq!(
        types(&columns),
        [("a", "integer"), ("b", "bigint"), ("c", "text")]
    );
    let columns = relation(
        "postgres",
        "WITH merged(value) AS ((SELECT 1) /* left */ UNION ALL (SELECT 2::bigint) /* right */)",
        "merged",
    );
    assert_eq!(types(&columns), [("value", "bigint")]);
    let columns = relation(
        "postgres",
        "WITH merged(value) AS (SELECT 1 UNION SELECT NULL)",
        "merged",
    );
    assert!(columns[0].nullable);
}

#[test]
fn mixed_physical_derived_and_unnamed_star_slots_keep_from_order() {
    let columns = infer_query_output_columns(
        "SELECT d.* FROM (\n SELECT u.id FROM users u\n) d",
        &catalog(),
        "oxide://query/lines.sql",
        40,
        Some("postgres"),
    );
    assert_eq!(
        columns[0].source_location,
        Some(("oxide://query/lines.sql".into(), 42))
    );
    for (body, expected) in [
        (
            "SELECT * FROM (SELECT 1::bigint AS extra) d CROSS JOIN users u",
            vec!["bigint", "integer", "bigint", "text"],
        ),
        (
            "SELECT * FROM users u CROSS JOIN (SELECT 1::bigint AS extra) d",
            vec!["integer", "bigint", "text", "bigint"],
        ),
        (
            "SELECT * FROM (SELECT COUNT(*) OVER (), u.id, u.name FROM users u) d",
            vec!["bigint", "integer", "text"],
        ),
    ] {
        let names = (0..expected.len())
            .map(|i| format!("output_{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let columns = relation(
            "postgres",
            &format!("WITH merged({names}) AS ({body})"),
            "merged",
        );
        assert_eq!(
            columns
                .iter()
                .map(|column| column.data_type.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{body}"
        );
    }
}

fn balanced_union(leaves: usize) -> String {
    if leaves == 1 {
        return "SELECT 1".into();
    }
    format!(
        "({}) UNION ALL ({})",
        balanced_union(leaves / 2),
        balanced_union(leaves - leaves / 2)
    )
}

#[test]
fn nested_query_and_branch_budgets_do_not_publish_partial_types() {
    for (count, expected) in [(64, "integer"), (65, "unknown")] {
        let columns = relation(
            "postgres",
            &format!("WITH merged(value) AS ({})", balanced_union(count)),
            "merged",
        );
        assert_eq!(types(&columns), [("value", expected)], "{count} branches");
    }
    let body = format!("{}SELECT 1{}", "(".repeat(40), ")".repeat(40));
    let columns = relation(
        "postgres",
        &format!("WITH merged(value) AS ({body})"),
        "merged",
    );
    assert_eq!(types(&columns), [("value", "unknown")]);
}

#[test]
fn duplicate_and_unnameable_projections_are_renamed_before_deduplication() {
    let columns = relation(
        "postgres",
        "WITH renamed(first, second, third) AS (SELECT u.id, u.id, 1 + 2 FROM users u)",
        "renamed",
    );
    assert_eq!(
        types(&columns),
        [
            ("first", "integer"),
            ("second", "integer"),
            ("third", "integer")
        ]
    );
    let columns = infer_query_output_columns(
        "SELECT d.id AS result FROM (SELECT u.id,u.id FROM users u) d",
        &catalog(),
        "oxide://query/ambiguous.sql",
        1,
        Some("postgres"),
    );
    assert_eq!(types(&columns), [("result", "unknown")]);
    let columns = relation(
        "postgres",
        "WITH renamed(value) AS (SELECT DISTINCT u.id FROM users u)",
        "renamed",
    );
    assert_eq!(types(&columns), [("value", "integer")]);
}

#[test]
fn correlated_star_outputs_preserve_position_and_local_unknown_shadowing() {
    let columns = query_output::infer_query_output_columns_with_names(
        "SELECT d.* FROM users u CROSS JOIN LATERAL(SELECT u.*) d",
        &catalog(),
        "oxide://star.sql",
        1,
        Some("postgres"),
        &[],
    );
    assert_eq!(
        types(&columns),
        [("id", "integer"), ("large", "bigint"), ("name", "text")]
    );
    assert!(columns.iter().all(|column| !column.nullable));
    for sql in [
        "SELECT d.value FROM users u CROSS JOIN LATERAL(SELECT id AS value FROM unknown_table) d",
        "SELECT d.value FROM users u CROSS JOIN LATERAL(SELECT u.name AS value FROM (SELECT id FROM users) u) d",
    ] {
        let columns=infer_query_output_columns(sql,&catalog(),"oxide://unknown.sql",1,Some("postgres"));
        assert_eq!(types(&columns),[("value","unknown")],"{sql}");
    }
}
#[test]
fn outer_apply_and_left_lateral_null_extend_literal_outputs() {
    for (dialect, join, ty, nullable) in [
        ("sqlserver", "CROSS APPLY", "int", false),
        ("sqlserver", "OUTER APPLY", "int", true),
        ("postgres", "CROSS JOIN LATERAL", "integer", false),
        ("postgres", "LEFT JOIN LATERAL", "integer", true),
    ] {
        let suffix = if join.starts_with("LEFT") {
            " ON TRUE"
        } else {
            ""
        };
        let sql = format!("SELECT d.* FROM users u {join}(SELECT 1 AS value) d{suffix}");
        let columns = infer_query_output_columns(
            &sql,
            &catalog(),
            "oxide://nullability.sql",
            1,
            Some(dialect),
        );
        assert_eq!(types(&columns), [("value", ty)], "{sql}");
        assert_eq!(columns[0].nullable, nullable, "{sql}");
    }
}
#[test]
fn correlated_outputs_share_the_enclosing_branch_budget() {
    for (branches, ty) in [(63, "integer"), (64, "unknown")] {
        let body = balanced_union(branches).replace("SELECT 1", "SELECT u.id");
        let sql = format!("SELECT d.value FROM users u CROSS JOIN LATERAL({body}) d(value)");
        let columns =
            infer_query_output_columns(&sql, &catalog(), "oxide://budget.sql", 1, Some("postgres"));
        assert_eq!(types(&columns), [("value", ty)], "{branches} branches");
    }
}

#[test]
fn scalar_query_outputs_keep_single_column_type_and_conservative_nullability() {
    for (dialect, body, ty) in [
        (
            "postgres",
            "SELECT u.id FROM users u WHERE FALSE",
            "integer",
        ),
        ("mysql", "SELECT u.id FROM users u WHERE FALSE", "integer"),
        ("sqlserver", "SELECT u.id FROM users u WHERE 1=0", "integer"),
        ("postgres", "SELECT 1", "integer"),
    ] {
        let columns = infer_query_output_columns(
            &format!("SELECT ({body}) AS value"),
            &catalog(),
            "oxide://scalar.sql",
            1,
            Some(dialect),
        );
        assert_eq!(types(&columns), [("value", ty)], "{dialect}: {body}");
        assert!(columns[0].nullable, "empty row set can yield NULL");
    }
}
#[test]
fn scalar_queries_share_the_parent_branch_budget() {
    for (branches, ty) in [(63, "integer"), (64, "unknown")] {
        // Many sibling WITH bodies keep the AST valid without relying on the
        // shared grammar's unsupported nested parenthesized set-operation form.
        let declarations = (0..branches - 1)
            .map(|index| format!("c{index} AS (SELECT 1)"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT (WITH {declarations} SELECT 1) AS value");
        let columns =
            infer_query_output_columns(&sql, &catalog(), "oxide://scalar.sql", 1, Some("postgres"));
        assert_eq!(types(&columns), [("value", ty)], "{branches}");
    }
    let sql = format!(
        "SELECT {}",
        (0..64)
            .map(|index| format!("(SELECT 1) AS c{index}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let columns =
        infer_query_output_columns(&sql, &catalog(), "oxide://scalar.sql", 1, Some("postgres"));
    assert_eq!(columns.len(), 64);
    assert!(columns[..63]
        .iter()
        .all(|column| column.data_type == "integer"));
    assert_eq!(columns[63].data_type, "unknown");
}
#[test]
fn scalar_depth_is_bounded_without_losing_explicit_output_name() {
    for (levels, ty) in [(5, "integer"), (33, "unknown")] {
        let mut expression = "1".to_string();
        for _ in 0..levels {
            expression = format!("(SELECT {expression})");
        }
        let sql = format!("SELECT {expression} AS value");
        let columns =
            infer_query_output_columns(&sql, &catalog(), "oxide://scalar.sql", 1, Some("postgres"));
        assert_eq!(types(&columns), [("value", ty)], "{levels}");
    }
}
#[test]
fn postgres_untyped_string_literals_follow_scalar_and_case_context() {
    for (expression, ty) in [
        ("COALESCE((SELECT u.id), '1')", "integer"),
        ("COALESCE((SELECT u.name), 'fallback')", "text"),
        (
            "CASE WHEN u.id>0 THEN (SELECT u.id) ELSE '1' END",
            "integer",
        ),
        ("COALESCE(NULL, 'fallback')", "text"),
        ("'literal'", "text"),
        ("COALESCE((SELECT '1'), 1)", "unknown"),
    ] {
        let columns = infer_query_output_columns(
            &format!("SELECT {expression} AS value FROM users u"),
            &catalog(),
            "oxide://scalar.sql",
            1,
            Some("postgres"),
        );
        assert_eq!(types(&columns), [("value", ty)], "{expression}");
        assert!(!columns[0].data_type.contains("__oxide"));
    }
}

#[test]
fn scalar_star_requires_known_single_column_arity() {
    let mut schema = catalog();
    let mut single = schema.tables[0].clone();
    single.name = "single".into();
    single.columns.retain(|column| column.name == "name");
    schema.tables.push(single);
    for (body, ty) in [
        ("SELECT * FROM single", "text"),
        ("SELECT * FROM users", "unknown"),
        ("SELECT * FROM missing", "unknown"),
    ] {
        let columns = infer_query_output_columns(
            &format!("SELECT ({body}) AS value"),
            &schema,
            "oxide://scalar.sql",
            1,
            Some("postgres"),
        );
        assert_eq!(types(&columns), [("value", ty)], "{body}");
    }
}

#[test]
fn scalar_nested_set_queries_keep_shared_branch_budget() {
    for (branches, ty) in [(63, "integer"), (64, "unknown")] {
        let body = balanced_union(branches).replace("SELECT 1", "SELECT u.id");
        let sql = format!("SELECT ({body}) AS value FROM users u");
        let columns = infer_query_output_columns(
            &sql,
            &catalog(),
            "oxide://grammar.sql",
            1,
            Some("postgres"),
        );
        assert_eq!(types(&columns), [("value", ty)], "{branches}");
    }
}

#[test]
fn mysql_and_tsql_integer_sets_follow_supported_engine_specific_promotions() {
    for (dialect, sql, ty) in [
        ("mysql", "SELECT 1 AS value UNION ALL SELECT 2", "bigint"),
        ("sqlserver", "SELECT 1 AS value UNION ALL SELECT 2", "int"),
        (
            "mysql",
            "SELECT u.large AS value FROM users u UNION ALL SELECT u.large FROM users u",
            "bigint",
        ),
        (
            "sqlserver",
            "SELECT u.large AS value FROM users u UNION ALL SELECT u.large FROM users u",
            "bigint",
        ),
        (
            "mysql",
            "SELECT u.id AS value FROM users u UNION ALL SELECT u.large FROM users u",
            "unknown",
        ),
        (
            "sqlserver",
            "SELECT u.id AS value FROM users u UNION ALL SELECT u.large FROM users u",
            "bigint",
        ),
        (
            "sqlserver",
            "SELECT CAST(1 AS DECIMAL(18,2)) AS value UNION ALL SELECT CAST(2 AS DECIMAL(20,3))",
            "unknown",
        ),
        (
            "mysql",
            "SELECT CAST(1 AS DECIMAL) AS value UNION ALL SELECT CAST(2 AS DECIMAL)",
            "unknown",
        ),
    ] {
        let columns =
            infer_query_output_columns(sql, &catalog(), "oxide://grammar.sql", 1, Some(dialect));
        assert_eq!(types(&columns), [("value", ty)], "{dialect}: {sql}");
        if sql.starts_with("SELECT 1 ") {
            assert!(!columns[0].nullable);
        }
    }
    let mut schema = catalog();
    schema.tables[0].columns.push(Column {
        name: "large_unsigned".into(),
        data_type: "bigint unsigned".into(),
        ..Default::default()
    });
    for (right, ty) in [("large_unsigned", "bigint unsigned"), ("large", "unknown")] {
        let sql = format!(
            "SELECT u.large_unsigned AS value FROM users u UNION ALL SELECT u.{right} FROM users u"
        );
        let columns =
            infer_query_output_columns(&sql, &schema, "oxide://grammar.sql", 1, Some("mysql"));
        assert_eq!(types(&columns), [("value", ty)]);
    }
}
