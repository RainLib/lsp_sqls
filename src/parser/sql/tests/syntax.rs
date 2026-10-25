use super::*;

#[test]
fn escape_is_a_bounded_pattern_operand_with_preserved_original_ranges() {
    let mut parser = SqlParser::new();
    for sql in [
        "SELECT 'oxide_1' LIKE 'oxide#_1' ESCAPE '#'",
        "SELECT CASE WHEN 'x' NOT ILIKE 'x' ESCAPE SUBSTR('##',1,1) THEN TRUE ELSE NULL END",
        "SELECT ('x' LIKE ('x' || '%') ESCAPE COALESCE('#','!')) AND TRUE",
        "SELECT 'x' SIMILAR TO 'x' ESCAPE '#'",
    ] {
        let tree = parser.parse_syntax_tree(sql).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "{}",
            tree.root_node().to_sexp()
        );
        let mut stack = vec![tree.root_node()];
        let mut found = 0;
        while let Some(node) = stack.pop() {
            if node.kind() == "binary_expression"
                && node
                    .child_by_field_name("operator")
                    .is_some_and(|op| op.kind() == "keyword_escape")
            {
                let parent = node.parent().unwrap();
                assert_eq!(parent.child_by_field_name("right").unwrap().id(), node.id());
                assert!(
                    node.child_by_field_name("left").is_some()
                        && node.child_by_field_name("right").is_some()
                );
                found += 1;
            }
            stack.extend(node.named_children(&mut node.walk()));
        }
        assert_eq!(found, 1, "{sql}");
    }
}

#[test]
fn pattern_operator_nodes_and_nested_type_arguments_keep_original_sources() {
    let mut parser = SqlParser::new();
    for sql in [
        "SELECT 'x' REGEXP '^x', 'x' NOT RLIKE 'y', 'x' GLOB 'x*', 'x' NOT GLOB 'y*'",
        "SELECT glob('x*','x'), regexp_matches('x','x'), \"glob\" FROM \"regexp\"",
        "SELECT CAST(NULL AS Nullable(String)), CAST(NULL AS LowCardinality(Nullable(String)))",
        "SELECT CAST(NULL AS Map(String, Array(UInt8)))",
    ] {
        let tree = parser.parse_syntax_tree(sql).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "{sql}: {}",
            tree.root_node().to_sexp()
        );
    }
}

#[test]
fn custom_numeric_type_modifiers_preserve_casts_arrays_and_invocations() {
    let mut parser = SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::Postgres,
    );
    for sql in [
        "SELECT CAST(1 AS Decimal64(2)) AS amount",
        "SELECT CAST(1 AS Decimal128(4)), CAST(1 AS Decimal256(8))",
        "SELECT CAST(NULL AS app.amount(12,2)[]) AS amounts",
        "SELECT NULL::app.amount(12,2)[]",
        "SELECT Decimal64(2), app.amount(12,2)",
        "SELECT CAST(1 AS \"Decimal64\"(2))",
    ] {
        let parsed = parser.parse(sql);
        assert!(
            parsed.diagnostics.is_empty(),
            "{sql}: {:?}",
            parsed.diagnostics
        );
        assert!(!parsed.tree.unwrap().root_node().has_error(), "{sql}");
    }
    for sql in [
        "SELECT CAST(1 AS Decimal64())",
        "SELECT CAST(1 AS Decimal64(2,))",
    ] {
        assert!(!parser.parse(sql).diagnostics.is_empty(), "{sql}");
    }
}

#[test]
fn arithmetic_normalization_keeps_source_offsets_and_protected_operands() {
    use crate::placeholder::SqlPlaceholderDialect;
    let sql = "SELECT -(i.amount) AS result FROM items i";
    let normalized =
        super::super::normalization::normalize_for_parser(sql, SqlPlaceholderDialect::Postgres);
    assert_eq!(normalized.len(), sql.len());
    let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::Postgres);
    let parsed = parser.parse(sql);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let protected="SELECT \"TRY_CAST\" - i.amount, 'TRY_CAST(x AS int)', i.amount /* -x */ - i.large FROM items i";
    let normalized = super::super::normalization::normalize_for_parser(
        protected,
        SqlPlaceholderDialect::Postgres,
    );
    assert_eq!(normalized, protected);
}

#[test]
fn arithmetic_adaptation_respects_udf_names_and_nested_postgres_comment_regions() {
    use crate::placeholder::SqlPlaceholderDialect;
    for sql in [
        "SELECT app . TRY_CAST(x, y) FROM items",
        "SELECT TRY_CAST(x,y) FROM items",
        "SELECT [TRY_CAST] FROM items",
        "SELECT $body$ TRY_CAST(x AS int) -x $body$",
        "/* outer /* inner */ TRY_CAST(x AS int) -x */ SELECT 1",
    ] {
        let normalized =
            super::super::normalization::normalize_for_parser(sql, SqlPlaceholderDialect::Postgres);
        assert_eq!(normalized, sql, "{sql}");
    }
}

#[test]
fn adapted_try_cast_and_unary_edits_match_fresh_parse_and_keep_utf16_ranges() {
    use crate::placeholder::SqlPlaceholderDialect;
    let old = "/* 🧭 */ SELECT TRY_CAST(i.label AS bigint) AS value FROM items i";
    let new = "/* 🧭 */ SELECT -(TRY_CAST(i.label AS decimal(18,4))) AS value FROM items i";
    let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::Generic);
    let first = parser.parse(old);
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let edited = parser.parse_incremental(new, old, first.tree.as_ref().unwrap());
    let mut fresh = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::Generic);
    let expected = fresh.parse(new);
    assert_eq!(
        edited.tree.as_ref().unwrap().root_node().to_sexp(),
        expected.tree.as_ref().unwrap().root_node().to_sexp()
    );
    assert_eq!(edited.diagnostics, expected.diagnostics);
    assert!(edited.diagnostics.is_empty());
    let byte = new.find("i.label").unwrap();
    let point = crate::position::byte_position_at_end(&new[..byte + 2]);
    let node = parser
        .get_node_at_position(edited.tree.as_ref().unwrap(), point)
        .unwrap();
    assert_eq!(parser.node_text(node, new), "label");
    assert_eq!(
        parser.node_range(node).start.character,
        new[..byte + 2].encode_utf16().count() as u32
    );
}

#[test]
fn adapted_try_cast_names_keep_full_token_ranges_and_function_casing() {
    let sql = "/* 🧭 */ SELECT try_cast(TRY_ AS bigint) AS result FROM items";
    let mut parser = SqlParser::new();
    let tree = parser.parse_syntax_tree(sql).unwrap();
    let tokens = parser.tokenize(&tree, sql);
    let field = crate::position::lsp_position_at_end(&sql[..sql.find("(TRY_").unwrap() + 1]);
    assert!(tokens
        .iter()
        .filter(|token| token.text == "TRY_")
        .all(|token| token.position == field));
    let function = tokens
        .iter()
        .find(|token| token.text == "try_cast")
        .expect("complete function token");
    assert_eq!(function.text.encode_utf16().count(), 8);
    let keyword = tree
        .root_node()
        .descendant_for_byte_range(sql.find("try_cast").unwrap(), sql.find("try_cast").unwrap())
        .unwrap();
    assert_eq!(
        parser.node_range(keyword).end.character - parser.node_range(keyword).start.character,
        8
    );
    let cased = crate::dialects::common::apply_formatter_function_case(sql, Some(true));
    assert_eq!(cased, sql.replacen("try_cast", "TRY_CAST", 1));
    let broken = parser.parse("SELECT TRY_CAST(label AS );");
    assert!(!broken.diagnostics.is_empty());
}

#[test]
fn postgres_postfix_casts_preserve_bindings_and_recover_array_type_brackets() {
    let mut parser = SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::Postgres,
    );
    for expr in [
        "1::bigint",
        "($1)::numeric(12,2)",
        "NULL::integer[]",
        "NULL::integer[][]",
        "NULL::integer[ /* type dimension */ ]",
    ] {
        let sql = format!("SELECT {expr} AS result");
        assert!(
            parser.parse(&sql).diagnostics.is_empty(),
            "{sql}: {:?}",
            parser.parse(&sql).diagnostics
        );
    }
}

#[test]
fn array_type_casts_preserve_real_errors_and_dialect_boundaries() {
    let mut parser = SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::Postgres,
    );
    for sql in [
        "SELECT value[1] FROM users",
        "SELECT CAST(value AS integer)[1] FROM users",
        "SELECT (value::integer)[''] FROM users",
    ] {
        let tree = parser.parse_syntax_tree(sql).unwrap();
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            assert!(!SqlParser::is_postfix_array_cast(node), "{sql}");
            stack.extend(node.children(&mut node.walk()));
        }
    }
    let sql = "SELECT (1 + )::integer[];";
    let parsed = parser.parse(sql);
    assert!(
        !parsed.diagnostics.is_empty(),
        "argument errors must survive: {}",
        parsed.tree.unwrap().root_node().to_sexp()
    );
    let mut parser =
        SqlParser::new_with_placeholder_dialect(crate::placeholder::SqlPlaceholderDialect::Mysql);
    assert!(!parser
        .parse("SELECT NULL::integer[];")
        .diagnostics
        .is_empty());
}

#[test]
fn incremental_parse_reuses_an_edited_tree_without_shifting_placeholders() {
    let original = "SELECT * FROM users WHERE id = $1";
    let updated = "SELECT email FROM users WHERE id = $1 AND active = true";
    let mut parser = SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::Postgres,
    );
    let original_result = parser.parse(original);
    let updated_result = parser.parse_incremental(
        updated,
        original,
        original_result.tree.as_ref().expect("initial tree"),
    );
    let tree = updated_result.tree.as_ref().expect("updated tree");
    assert_eq!(
        parser.extract_referenced_tables(tree, updated),
        vec!["users"]
    );
    assert!(updated_result.diagnostics.is_empty());

    let edit = minimal_input_edit("SELECT 😀", "SELECT 😀, id");
    assert_eq!(edit.start_byte, "SELECT 😀".len());
    assert_eq!(edit.old_end_position.column, "SELECT 😀".len());
}

#[test]
fn normalizes_quoted_identifier_paths() {
    assert_eq!(
        SqlParser::normalize_identifier(r#""public"."users""#),
        "public.users"
    );
    assert_eq!(
        SqlParser::normalize_identifier("`app`.`orders`"),
        "app.orders"
    );
    assert_eq!(
        SqlParser::normalize_identifier("[dbo].[customers];"),
        "dbo.customers"
    );
    assert_eq!(
        SqlParser::identifier_qualifier(r#""public"."calculate_score""#).as_deref(),
        Some("public")
    );
    assert_eq!(
        SqlParser::identifier_qualifier("catalog.public.calculate_score").as_deref(),
        Some("catalog.public")
    );
    assert_eq!(SqlParser::identifier_qualifier("calculate_score"), None);
}

#[test]
fn normalizes_sqlserver_catalog_and_default_schema_paths() {
    assert_eq!(
        SqlParser::normalize_identifier("[BarDB]..[orders]"),
        "BarDB..orders"
    );
    assert!(SqlParser::table_name_matches_with_namespace(
        "[ServerOne].[AppDb].[dbo].[Users]",
        Some("ServerOne"),
        Some("AppDb"),
        "dbo",
        "Users",
    ));
    assert!(!SqlParser::table_name_matches_with_catalog(
        "AppDb..Users",
        Some("AppDb"),
        "dbo",
        "Users",
    ));
    assert!(!SqlParser::table_name_matches_with_catalog(
        "[ServerOne].[OtherDb].[dbo].[Users]",
        Some("AppDb"),
        "dbo",
        "Users",
    ));

    let sql = "SELECT * FROM [ServerOne].[AppDb].[dbo].[Users] u JOIN BarDB..Orders o ON o.user_id = u.id";
    let mut parser = SqlParser::new();
    let tree = parser.parse(sql).tree.expect("SQL should parse");
    assert_eq!(
        parser.extract_referenced_tables(&tree, sql),
        vec![
            "ServerOne.AppDb.dbo.Users".to_string(),
            "BarDB..Orders".to_string(),
        ]
    );
    assert_eq!(
        SqlParser::column_qualifier_before_position(
            "SELECT * FROM BarDB..",
            position_at_end("SELECT * FROM BarDB.."),
        )
        .as_deref(),
        Some("BarDB.")
    );
}

#[test]
fn extracts_column_qualifier_before_cursor() {
    let alias_dot = "SELECT * FROM users u WHERE u.";
    assert_eq!(
        SqlParser::column_qualifier_before_position(alias_dot, position_at_end(alias_dot))
            .as_deref(),
        Some("u")
    );

    let alias_prefix = "SELECT * FROM users u WHERE u.na";
    assert_eq!(
        SqlParser::column_qualifier_before_position(alias_prefix, position_at_end(alias_prefix))
            .as_deref(),
        Some("u")
    );

    let qualified_dot = "SELECT * FROM public.users.";
    assert_eq!(
        SqlParser::column_qualifier_before_position(qualified_dot, position_at_end(qualified_dot))
            .as_deref(),
        Some("public.users")
    );

    let no_qualifier = "SELECT name FROM users";
    assert_eq!(
        SqlParser::column_qualifier_before_position(no_qualifier, position_at_end(no_qualifier)),
        None
    );
}

#[test]
fn matches_schema_qualified_table_names() {
    assert!(SqlParser::table_name_matches("users", "public", "users"));
    assert!(SqlParser::table_name_matches(
        r#""public"."users""#,
        "public",
        "users"
    ));
    assert!(SqlParser::table_name_matches(
        "`app`.`orders`",
        "app",
        "orders"
    ));
    assert!(!SqlParser::table_name_matches(
        "archive.users",
        "public",
        "users"
    ));
}

#[test]
fn cross_and_outer_apply_parse_without_losing_original_keywords() {
    for introducer in ["CROSS", "OUTER"] {
        let sql = format!(
            "/* 🧭 */\r\nSELECT d.total FROM orders o {introducer} APPLY(SELECT o.total) d"
        );
        let mut parser = SqlParser::new();
        let parsed = parser.parse(&sql);
        assert!(
            parsed.diagnostics.is_empty(),
            "{sql}: {:?}",
            parsed.diagnostics
        );
        assert_eq!(parsed.source, sql);
        let at = sql.find("APPLY").unwrap() + 2;
        let tree = parsed.tree.as_ref().unwrap();
        let node = parser
            .get_node_at_position(tree, crate::position::byte_position_at_end(&sql[..at]))
            .unwrap();
        assert_eq!(parser.node_text(node, &sql), "APPLY");
        let range = parser.node_range(node);
        assert_eq!(range.end.character - range.start.character, 5);
        assert_eq!(range.start.line, 1);
    }
}
#[test]
fn apply_adapter_protects_quotes_comments_and_non_join_identifiers() {
    use crate::placeholder::SqlPlaceholderDialect;
    for sql in [
        "SELECT 'OUTER APPLY(x)'",
        "SELECT \"CROSS APPLY\" FROM users",
        "SELECT CROSS_APPLY(id) FROM users",
        "SELECT public.apply(id) FROM users",
        "SELECT 1 /* OUTER APPLY */",
    ] {
        assert_eq!(
            super::super::normalization::normalize_for_parser(sql, SqlPlaceholderDialect::Generic),
            sql
        );
    }
    let sql = "SELECT d.id FROM users u OUTER /* note */ APPLY(SELECT u.id) d";
    let mut parser = SqlParser::new();
    assert!(parser.parse(sql).diagnostics.is_empty());
}
#[test]
fn apply_adaptation_keeps_inner_expression_diagnostics() {
    let mut parser = SqlParser::new();
    assert!(!parser
        .parse("SELECT * FROM users u OUTER APPLY(SELECT u.id +) d")
        .diagnostics
        .is_empty());
    assert!(!parser
        .parse("SELECT * FROM users u CROSS APPLY(SELECT FROM) d")
        .diagnostics
        .is_empty());
}

#[test]
fn from_recovery_does_not_reject_quoted_or_qualified_identifiers() {
    let mut parser = SqlParser::new();
    for sql in ["SELECT \"FROM\" FROM users", "SELECT u.from FROM users u"] {
        assert!(parser.parse(sql).diagnostics.is_empty(), "{sql}");
    }
    assert!(!parser.parse("SELECT FROM;").diagnostics.is_empty());
}
