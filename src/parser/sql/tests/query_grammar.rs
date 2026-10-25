use super::*;
#[test]
fn predicates_keep_operator_tokens_and_real_row_sources() {
    for sql in [
        "SELECT u.id IS DISTINCT FROM NULL AS value FROM users u",
        "SELECT DISTINCT u.id IS NOT DISTINCT /* FROM */ FROM NULL AS value FROM users u",
        "SELECT u.id <=> NULL AS value FROM users u",
    ] {
        let mut parser = SqlParser::new();
        let result = parser.parse(sql);
        assert!(
            result.diagnostics.is_empty(),
            "{sql}: {:?}",
            result.diagnostics
        );
        let tree = result.tree.unwrap();
        assert_eq!(parser.extract_referenced_tables(&tree, sql), vec!["users"]);
        assert_eq!(
            parser.extract_aliases(&tree, sql).get("u"),
            Some(&"users".to_string())
        );
    }
    for (sql, keyword) in [
        ("SELECT 1 IS DISTINCT FROM NULL FROM users", "FROM NULL"),
        (
            "SELECT 1 IS NOT DISTINCT /* FROM */ FROM NULL FROM users",
            "FROM NULL",
        ),
    ] {
        let masked = SqlParser::mask_sql_noise(sql);
        let position = masked.find(keyword).unwrap();
        assert!(SqlParser::is_distinct_from_keyword(&masked, position));
        assert!(!SqlParser::is_distinct_from_keyword(
            &masked,
            masked.rfind("FROM").unwrap()
        ));
    }
}
#[test]
fn select_clauses_do_not_require_from() {
    for sql in [
        "SELECT 1 WHERE FALSE",
        "SELECT 1 WHERE 1=1 ORDER BY 1 LIMIT 1",
        "SELECT (SELECT 1 WHERE FALSE UNION ALL SELECT 2) AS value",
    ] {
        let result = SqlParser::new().parse(sql);
        assert!(
            result.diagnostics.is_empty(),
            "{sql}: {:?}",
            result.diagnostics
        );
    }
}
#[test]
fn nested_set_operands_retain_grouping_and_original_coordinates() {
    let sql="/* 😀 */\r\nSELECT ((SELECT 1 UNION ALL SELECT 2) UNION ALL (SELECT 3 UNION ALL SELECT 4)) AS value";
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let tree = result.tree.unwrap();
    assert_eq!(
        tree.root_node()
            .to_sexp()
            .matches("(set_operation ")
            .count(),
        3
    );
    let offset = sql.rfind('4').unwrap();
    let position = crate::position::byte_position_at_end(&sql[..offset]);
    let node = parser.get_node_at_position(&tree, position).unwrap();
    assert_eq!(parser.node_text(node, sql), "4");
    assert_eq!(
        parser.node_range(node).start,
        crate::position::lsp_position_at_end(&sql[..offset])
    );
}
#[test]
fn query_grammar_keeps_real_errors() {
    assert!(
        SqlParser::new()
            .parse("SELECT 1 WHERE")
            .diagnostics
            .is_empty(),
        "unfinished editor input keeps existing EOF policy"
    );
    for sql in [
        "SELECT 1 WHERE;",
        "SELECT 1 WHERE ( + )",
        "SELECT (SELECT 1 UNION ALL) AS value",
        "SELECT ((SELECT 1) UNION ALL ()) AS value",
    ] {
        assert!(!SqlParser::new().parse(sql).diagnostics.is_empty(), "{sql}");
    }
}
#[test]
fn parenthesized_expressions_and_relations_stay_distinct() {
    for sql in [
        "SELECT (1+2) AS value",
        "SELECT d.id FROM (SELECT 1 AS id) d WHERE d.id>0",
        "SELECT 1 FROM users WHERE id=1",
    ] {
        let result = SqlParser::new().parse(sql);
        assert!(
            result.diagnostics.is_empty(),
            "{sql}: {:?}",
            result.diagnostics
        );
    }
}
