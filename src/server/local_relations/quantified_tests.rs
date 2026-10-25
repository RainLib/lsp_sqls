use super::predicate_tests::{empty_schema, output};
use super::*;

#[test]
fn quantified_subqueries_keep_fixed_types_and_conservative_nullability() {
    for (dialect, ty) in [("postgres", "boolean"), ("mysql", "bigint")] {
        for quantifier in ["ANY", "SOME", "ALL"] {
            for operator in ["=", "<>", "!=", "<", ">", "<=", ">="] {
                let expression =
                    format!("1 {operator} {quantifier}(SELECT NULL WHERE 1=0 UNION ALL SELECT 2)");
                let column = output(dialect, &expression);
                assert_eq!(column.data_type, ty, "{dialect}: {expression}");
                assert!(column.nullable, "{expression}");
            }
        }
    }
}

#[test]
fn postgres_array_quantifiers_distinguish_empty_null_and_nonnull_arrays() {
    for (expression, nullable) in [
        ("1 = ANY(ARRAY[1,2])", false),
        ("1 <> ALL(ARRAY[1,2,NULL])", true),
        ("NULL = ANY(ARRAY[1,2])", true),
        ("1 = ANY(NULL::integer[])", true),
        ("1 = ANY(CAST(NULL AS integer[]))", true),
        ("NULL = ANY(ARRAY[]::integer[])", false),
        ("NULL = ANY(ARRAY[]::integer[ /* dimension */ ][])", false),
        ("NULL <> ALL(CAST(ARRAY[] AS integer[]))", false),
        ("1 = SOME((ARRAY[1,2]::integer[]))", false),
        ("TRUE = ALL(ARRAY[TRUE,FALSE])", false),
        ("1 = ANY(ARRAY[ARRAY[1,2],ARRAY[3,4]])", true),
        ("1 = ANY(ARRAY(SELECT 1 UNION ALL SELECT NULL))", true),
    ] {
        let column = output("postgres", expression);
        assert_eq!(column.data_type, "boolean", "{expression}");
        assert_eq!(column.nullable, nullable, "{expression}");
    }
}

#[test]
fn quantified_patterns_regex_and_search_keep_boolean_results() {
    for (expression, nullable) in [
        ("'oxide' LIKE ANY(ARRAY['ox%','db%'])", false),
        ("'oxide' NOT ILIKE ALL(ARRAY['x%','z%'])", false),
        ("'oxide' ~ ALL(ARRAY['^ox','de$'])", false),
        ("'oxide' !~* SOME(ARRAY['DBX',NULL])", true),
        (
            "CAST('oxide' AS tsvector) @@ ANY(ARRAY[CAST('oxide' AS tsquery)])",
            true,
        ),
        (
            "CAST('{}' AS jsonb) @@ ANY(ARRAY[CAST('$.key' AS jsonpath)])",
            true,
        ),
    ] {
        let column = output("postgres", expression);
        assert_eq!(column.data_type, "boolean", "{expression}");
        assert_eq!(column.nullable, nullable, "{expression}");
    }
}

#[test]
fn row_components_keep_arity_types_and_nullability() {
    for (dialect, ty) in [
        ("postgres", "boolean"),
        ("mysql", "bigint"),
        ("sqlite", "integer"),
    ] {
        for (expression, nullable) in [
            ("(1,2) = (1,2)", false),
            ("(1,NULL) <> (1,2)", true),
            ("((1,2)) >= ((2,3))", false),
            ("(1,'oxide') < (2,'dbx')", false),
            ("(1,2) BETWEEN (0,0) AND (3,3)", false),
            ("(1,2) = (SELECT 1,2)", true),
        ] {
            let column = output(dialect, expression);
            assert_eq!(column.data_type, ty, "{dialect}: {expression}");
            assert_eq!(column.nullable, nullable, "{dialect}: {expression}");
        }
    }
    for expression in ["ROW(1,'oxide')=ROW(2,'dbx')", "ROW(1)=ROW(2)"] {
        let column = output("postgres", expression);
        assert_eq!(column.data_type, "boolean");
        assert!(!column.nullable);
    }
}

#[test]
fn compound_operands_do_not_fabricate_scalar_or_incompatible_types() {
    for (dialect, expression) in [
        ("postgres", "(SELECT 1,2)"),
        ("postgres", "(1,2)=(1,2,3)"),
        ("postgres", "(1,2)=1"),
        (
            "postgres",
            "ROW(1,CAST(NULL AS custom_type))=ROW(1,CAST(NULL AS custom_type))",
        ),
        ("postgres", "1=ANY(1)"),
        ("postgres", "1=ANY(ARRAY[])"),
        ("postgres", "1=ANY(mystery())"),
        ("mysql", "1=ANY(ARRAY[1,2])"),
        ("sqlite", "1=ANY(SELECT 1)"),
        ("sqlserver", "1=ALL(SELECT 1)"),
    ] {
        assert_eq!(
            output(dialect, expression).data_type,
            "unknown",
            "{dialect}: {expression}"
        );
    }
}

#[test]
fn quantified_bodies_preserve_the_neighboring_scalar_budget() {
    let branches = (0..100)
        .map(|_| "SELECT 1")
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    for quantifier in ["ANY", "SOME", "ALL"] {
        let columns = infer_query_output_columns(
            &format!("SELECT 1={quantifier}({branches}) AS predicate, (SELECT 1) AS scalar"),
            &empty_schema(),
            "oxide://quantified.sql",
            1,
            Some("postgres"),
        );
        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0].data_type, "boolean");
        assert_eq!(columns[1].data_type, "integer");
    }
}

fn balanced_rows(branches: usize) -> String {
    if branches == 1 {
        return "SELECT 1,2".into();
    }
    format!(
        "({}) UNION ALL ({})",
        balanced_rows(branches / 2),
        balanced_rows(branches - branches / 2)
    )
}

#[test]
fn row_query_outputs_share_existing_branch_budget() {
    for (branches, ty) in [(63, "boolean"), (64, "unknown")] {
        let column = output("postgres", &format!("(1,2)=({})", balanced_rows(branches)));
        assert_eq!(column.data_type, ty, "{branches}");
    }
}

#[test]
fn compound_lists_are_bounded_without_partial_row_shapes() {
    let list = (0..300).map(|_| "1").collect::<Vec<_>>().join(",");
    let column = output("postgres", &format!("1=ANY(ARRAY[{list}])"));
    assert_eq!(column.data_type, "boolean");
    assert!(column.nullable);
    assert_eq!(
        output("postgres", &format!("({list})=({list})")).data_type,
        "unknown"
    );
    let query = format!("SELECT {list}");
    assert_eq!(
        output("postgres", &format!("({query})=({list})")).data_type,
        "unknown"
    );
}

#[test]
fn retained_row_types_have_a_byte_budget_and_leave_narrow_neighbors_available() {
    let mut schema = empty_schema();
    schema.tables.push(Table {
        name: "users".into(),
        columns: vec![Column {
            name: "opaque".into(),
            data_type: "x".repeat(140_000),
            ..Default::default()
        }],
        ..Default::default()
    });
    let columns = infer_query_output_columns("SELECT (u.opaque,u.opaque)=(SELECT x.opaque,x.opaque FROM users x) AS wide, (SELECT 1) AS scalar FROM users u", &schema, "oxide://quantified.sql", 1, Some("mysql"));
    assert_eq!(columns.len(), 2);
    assert_eq!(columns[0].data_type, "unknown");
    assert_eq!(columns[1].data_type, "bigint");
}
