use super::*;

#[test]
fn quantified_and_row_comparisons_keep_structured_operands() {
    for (expression, right_kind) in [
        ("1 = ANY(ARRAY[1,2,NULL])", "unary_expression"),
        ("1 <> ALL(ARRAY[1,2])", "unary_expression"),
        ("1 > SOME(SELECT id FROM orders)", "unary_expression"),
        ("1 <= ALL(SELECT id FROM orders)", "unary_expression"),
        ("(1,'oxide') = (2,'dbx')", "list"),
        ("ROW(1,'oxide') < ROW(2,'dbx')", "invocation"),
        ("(1,'oxide') IN (SELECT id,name FROM users)", "subquery"),
    ] {
        let sql = format!("SELECT {expression} AS value");
        let result = SqlParser::new().parse(&sql);
        let tree = result.tree.unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "{sql}: {:?}",
            result.diagnostics
        );
        let node = tree
            .root_node()
            .named_descendant_for_byte_range(7, 7 + expression.len())
            .unwrap();
        assert_eq!(node.kind(), "binary_expression", "{sql}");
        assert_eq!(
            node.child_by_field_name("right").unwrap().kind(),
            right_kind,
            "{sql}"
        );
    }
}

#[test]
fn postgres_array_casts_have_complete_nodes_inside_complex_expressions() {
    let mut parser = SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::Postgres,
    );
    for sql in [
        "SELECT 1=ANY(NULL::integer[])",
        "SELECT NULL=ANY(ARRAY[]::integer[])",
        "SELECT COALESCE(NULL::integer[], ARRAY[1])",
        "SELECT (ARRAY[1,2]::integer[])[1]",
        "SELECT ARRAY[1,2]::integer[3]",
        "SELECT NULL::integer[ /* dimension */ ][]",
    ] {
        let result = parser.parse(sql);
        let tree = result.tree.unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "{sql}: {:?}",
            result.diagnostics
        );
        assert!(
            !tree.root_node().has_error(),
            "{sql}: {}",
            tree.root_node().to_sexp()
        );
    }
}

#[test]
fn postfix_casts_preserve_arithmetic_and_subscript_binding() {
    let mut parser = SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::Postgres,
    );
    for (expression, expected) in [
        ("1+2::integer*3", "2::integer"),
        ("(1+2)::integer[]", "(1+2)::integer[]"),
        ("(ARRAY[1,2]::integer[])[1]", "ARRAY[1,2]::integer[]"),
        ("ARRAY[1,2]::integer[3][4]", "ARRAY[1,2]::integer[3][4]"),
    ] {
        let sql = format!("SELECT {expression}");
        let result = parser.parse(&sql);
        assert!(
            result.diagnostics.is_empty(),
            "{sql}: {:?}",
            result.diagnostics
        );
        let tree = result.tree.unwrap();
        let mut stack = vec![tree.root_node()];
        let mut casts = vec![];
        while let Some(node) = stack.pop() {
            if node.kind() == "cast" {
                casts.push(node.utf8_text(sql.as_bytes()).unwrap());
            }
            stack.extend(node.named_children(&mut node.walk()));
        }
        assert_eq!(casts, [expected], "{sql}: {}", tree.root_node().to_sexp());
    }
}

#[test]
fn identifier_tokens_distinguish_postgres_arrays_and_bracket_identifiers() {
    use crate::placeholder::SqlPlaceholderDialect;
    for prefix in [
        "SELECT ARRAY[COALESCE(u.na",
        "SELECT ARRAY [u.na",
        "SELECT u.ids[u.na",
        "SELECT ARRAY[ARRAY[u.na",
    ] {
        assert_eq!(
            SqlParser::identifier_path_token_for_dialect(prefix, SqlPlaceholderDialect::Postgres),
            Some("u.na"),
            "{prefix}"
        );
    }
    for dialect in [
        SqlPlaceholderDialect::Generic,
        SqlPlaceholderDialect::SqlServer,
    ] {
        assert_eq!(
            SqlParser::identifier_path_token_for_dialect("SELECT [u].[odd]]na", dialect),
            Some("[u].[odd]]na")
        );
    }
    assert_eq!(
        SqlParser::identifier_path_token_for_dialect(
            "SELECT ARRAY[u.\"odd[.na",
            SqlPlaceholderDialect::Postgres
        ),
        Some("u.\"odd[.na")
    );
}
