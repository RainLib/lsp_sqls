use super::*;
fn complete(sql: &str, profile: &str) -> Option<Vec<CompletionItem>> {
    completions(
        sql,
        crate::position::lsp_position_at_end(sql),
        profile,
        || None,
    )
}

#[test]
fn opaque_regions_and_quoted_is_members_do_not_become_operators() {
    for (sql, profile) in [
        ("SELECT t.\"IS\" ", "postgres"),
        ("SELECT t.IS ", "postgres"),
        ("SELECT [IS] ", "sqlserver"),
        ("SELECT `IS` ", "mysql"),
        ("SELECT 'IS' ", "postgres"),
        ("SELECT $$a IS$$ ", "postgres"),
        ("SELECT 1 AS IS ", "postgres"),
    ] {
        assert!(complete(sql, profile).is_none(), "{sql}");
    }
    for (sql, profile) in [
        ("SELECT flag IS 'NU", "postgres"),
        ("SELECT flag IS /* NU", "postgres"),
        ("SELECT flag IS -- NU", "postgres"),
        ("SELECT flag IS # NU", "mysql"),
    ] {
        assert!(complete(sql, profile).unwrap().is_empty(), "{sql}");
    }
}

#[test]
fn sqlite_keeps_arbitrary_is_operands_and_distinct_from_keeps_the_expression_service() {
    for sql in [
        "SELECT value IS ",
        "SELECT value IS NOT ",
        "SELECT value IS other ",
    ] {
        assert!(complete(sql, "sqlite").is_none(), "{sql}");
    }
    for profile in ["postgres", "duckdb", "sqlite", "sqlserver", "clickhouse"] {
        for sql in [
            "SELECT value IS DISTINCT FROM ",
            "SELECT value IS NOT DISTINCT FROM ",
            "SELECT value IS NOT DISTINCT FROM q.field",
        ] {
            assert!(complete(sql, profile).is_none(), "{profile}: {sql}");
        }
    }
}

#[test]
fn comments_nested_scalars_and_unicode_keep_keyword_slots() {
    for sql in [
        "SELECT COALESCE(flag, FALSE) IS ",
        "SELECT CASE WHEN x=1 THEN TRUE ELSE FALSE END IS ",
        "SELECT (SELECT MAX(x) FROM t) IS NOT ",
        "SELECT \"字段\" IS /*😀 NOT*/ N",
    ] {
        let items = complete(sql, "postgres").unwrap();
        assert!(
            items
                .iter()
                .any(|item| item.label == "NULL" || item.label == "NOT NULL"),
            "{sql}"
        );
        assert!(items
            .iter()
            .all(|item| item.kind == Some(CompletionItemKind::KEYWORD)));
    }
}

#[test]
fn predicate_scan_bounds_and_unknown_profiles_delegate_without_truncating_sql() {
    assert!(complete(
        &format!("{}SELECT value IS ", " ".repeat(524289)),
        "postgres"
    )
    .is_none());
    assert!(complete("SELECT value IS ", "oracle").is_none());
    assert!(complete("SELECT value IS ", "mongodb").is_none());
    assert!(complete(&format!("SELECT value IS {}", "a".repeat(2048)), "postgres").is_none());
}

#[test]
fn static_keyword_transitions_and_opaque_regions_never_load_metadata() {
    for sql in [
        "SELECT value IS NOT DISTINCT ",
        "SELECT value IS /* open",
        "SELECT value IS 'open",
    ] {
        assert!(completions(
            sql,
            crate::position::lsp_position_at_end(sql),
            "postgres",
            || panic!("static syntax cannot read metadata")
        )
        .is_some());
    }
    assert!(completions(
        "SELECT value ",
        Position::new(0, 13),
        "postgres",
        || panic!("ordinary expressions do not need this scanner's metadata")
    )
    .is_none());
}

#[test]
fn large_suffix_documents_keep_keyword_hints_without_probing_catalog_types() {
    let prefix = "SELECT flag IS ";
    let sql = format!("{prefix};{}", " ".repeat(600_000));
    let items = completions(
        &sql,
        crate::position::lsp_position_at_end(prefix),
        "postgres",
        || panic!("a short prefix must not clone or probe the oversized full document"),
    )
    .unwrap();
    assert!(items.iter().any(|item| item.label == "NULL"));
}

#[test]
fn coercing_profiles_do_not_load_unused_metadata_for_is_keywords() {
    for profile in ["mysql", "mariadb", "hive", "duckdb"] {
        let sql = "SELECT flag IS ";
        let items = completions(
            sql,
            crate::position::lsp_position_at_end(sql),
            profile,
            || panic!("{profile} keyword syntax does not need catalog metadata"),
        )
        .unwrap();
        assert!(items.iter().any(|item| item.label == "NULL"));
    }
}

#[test]
fn explicit_scalar_subjects_keep_postgres_truth_hints_typed() {
    for (expression, truth) in [
        ("CAST(1 AS INTEGER)", false),
        ("CAST(TRUE AS BOOLEAN)", true),
        ("CASE WHEN TRUE THEN 1 ELSE 2 END", false),
        ("COALESCE(1, 2)", false),
        ("LOWER('value')", false),
        ("(1 + 2)", false),
        ("CAST('value' AS TEXT)", false),
        ("'true'", true),
        ("TRUE", true),
        ("unknown_function(1)", true),
    ] {
        let sql = format!("SELECT {expression} IS ");
        let items = complete(&sql, "postgres").unwrap();
        let mut parser = SqlParser::new();
        let tree = parser.parse(&sql).tree.unwrap();
        assert_eq!(
            items.iter().any(|item| item.label == "TRUE"),
            truth,
            "{sql}: {}",
            tree.root_node().to_sexp()
        );
    }
}

#[test]
fn oversized_word_suffixes_delegate_before_metadata_and_edit_generation() {
    for count in [1025, 600_000] {
        let prefix = "SELECT flag IS ";
        let sql = format!("{prefix}{}", "a".repeat(count));
        assert!(completions(
            &sql,
            crate::position::lsp_position_at_end(prefix),
            "postgres",
            || panic!("oversized replacement words must not read metadata")
        )
        .is_none());
    }
}

#[test]
fn comparison_type_probes_reject_invalid_ranges_and_oversized_documents() {
    let schema = super::super::document_semantics::local_schema(
        "SELECT 1",
        Position::new(0, 8),
        "",
        "postgres",
    );
    for (sql, range, cursor) in [
        ("SELECT 1", 9..10, 8),
        ("😀 IS ", 1..6, 8),
        ("SELECT 1", std::ops::Range { start: 4, end: 2 }, 8),
    ] {
        assert!(super::super::local_relations::infer_comparison_left_type(
            sql, range, &schema, "", cursor, "postgres", None
        )
        .is_none());
    }
    let sql = format!("SELECT 1 IS ;{}", " ".repeat(600_000));
    assert!(super::super::local_relations::infer_comparison_left_type(
        &sql,
        9..11,
        &schema,
        "",
        12,
        "postgres",
        None
    )
    .is_none());
}

#[test]
fn closed_statement_subjects_keep_cast_types_at_the_keyword_cursor() {
    let prefix = "/*😀 字段*/ SELECT * FROM jobs j WHERE CAST(j.amount AS INTEGER) IS ";
    let sql = prefix.to_string() + ";";
    let items = completions(
        &sql,
        crate::position::lsp_position_at_end(prefix),
        "postgres",
        || None,
    )
    .unwrap();
    let mut parser = SqlParser::new();
    let tree = parser.parse(&sql).tree.unwrap();
    assert!(
        !items.iter().any(|item| item.label == "TRUE"),
        "{}",
        tree.root_node().to_sexp()
    );
}
