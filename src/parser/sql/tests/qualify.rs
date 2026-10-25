use super::*;

#[test]
fn qualify_grammar_keeps_clauses_row_sources_and_real_syntax_errors() {
    for sql in [
        "SELECT row_number() OVER(ORDER BY total) AS rn FROM orders QUALIFY rn=1 ORDER BY total LIMIT 3",
        "SELECT total,count() OVER w AS c FROM orders WINDOW w AS (PARTITION BY total) QUALIFY c>1",
        "SELECT total FROM orders QUALIFY row_number() OVER(ORDER BY total)=1",
        "WITH ranked AS (SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1) SELECT rn FROM ranked",
    ] {
        let mut parser=SqlParser::new(); let parsed=parser.parse(sql);
        assert!(parsed.diagnostics.is_empty(),"{sql}: {:?}",parsed.diagnostics);
        assert!(parsed.tree.as_ref().unwrap().root_node().to_sexp().contains("(qualify "));
        assert!(parser.extract_referenced_tables(parsed.tree.as_ref().unwrap(),sql).iter().any(|s|s=="orders"),"{sql}");
    }
    for sql in [
        "SELECT row_number() OVER() AS rn QUALIFY",
        "SELECT row_number() OVER() AS rn QUALIFY rn=",
        "SELECT row_number() OVER() AS rn QUALIFY (rn=1",
    ] {
        let parsed = SqlParser::new().parse(sql);
        // Existing editor policy suppresses some trailing incomplete diagnostics;
        // recovery must nevertheless retain erroneous original syntax in the tree.
        assert!(parsed.tree.unwrap().root_node().has_error(), "{sql}");
    }
}

#[test]
fn qualify_range_uses_own_query_branch_and_ignores_completed_nested_keywords() {
    for (marked, expected) in [
        ("SELECT row_number() OVER() AS outer_rank FROM orders QUALIFY EXISTS(SELECT row_number() OVER() AS inner_rank QUALIFY <cursor>)", "SELECT row_number() OVER() AS inner_rank "),
        ("WITH c AS (SELECT row_number() OVER() AS before_rank QUALIFY before_rank=1) SELECT row_number() OVER() AS own_rank FROM c QUALIFY <cursor>", "SELECT row_number() OVER() AS own_rank FROM c "),
        ("SELECT row_number() OVER() AS first_rank QUALIFY first_rank=1 UNION ALL SELECT row_number() OVER() AS own_rank QUALIFY <cursor>", "SELECT row_number() OVER() AS own_rank "),
    ] {
        let cursor=marked.find("<cursor>").unwrap();let sql=marked.replace("<cursor>","");
        let range=SqlParser::qualify_query_range(&sql,cursor).unwrap();assert_eq!(&sql[range],expected,"{marked}");
    }
    for marked in [
        "SELECT 'QUALIFY' AS q<cursor>",
        "SELECT 1 /* QUALIFY */ AS q<cursor>",
        "SELECT row_number() OVER() AS rn QUALIFY rn=1 ORDER BY <cursor>",
    ] {
        let cursor = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        assert!(
            SqlParser::qualify_query_range(&sql, cursor).is_none(),
            "{marked}"
        );
    }
}

#[test]
fn qualify_open_predicate_groups_offer_expressions_after_a_completed_comparison() {
    for clause in ["WHERE", "QUALIFY"] {
        for tail in ["rn=1 AND (", "rn=1 AND ", "rn=1 OR ("] {
            let sql = format!("SELECT row_number() OVER() AS rn FROM orders {clause} {tail}");
            assert_eq!(
                analyzed_context_at_end(&sql),
                CompletionContext::WhereClause,
                "{sql}"
            );
        }
    }
}

#[test]
fn qualify_scope_budgets_preserve_unicode_boundaries_without_partial_results() {
    let sql = "/* 😀 */\r\nSELECT row_number() OVER() AS 排名 QUALIFY 排名=1";
    let range = SqlParser::qualify_query_range(sql, sql.len() - 2).unwrap();
    assert!(sql[range].starts_with("SELECT"));
    let middle = sql.find('名').unwrap() + 1;
    assert!(SqlParser::qualify_query_range(sql, middle).is_none());
    assert!(SqlParser::qualify_query_range(sql, sql.len() + 1).is_none());
    let huge = format!(
        "SELECT row_number() OVER() AS rn /*{}*/ QUALIFY rn=1",
        "x".repeat(512 * 1024)
    );
    assert!(SqlParser::qualify_query_range(&huge, huge.len()).is_none());
    let tokens = format!("SELECT {} QUALIFY rn=1", "x ".repeat(32769));
    assert!(SqlParser::qualify_query_range(&tokens, tokens.len()).is_none());
}
