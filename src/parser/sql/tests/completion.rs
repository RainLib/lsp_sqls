use super::*;

#[test]
fn partial_member_prefix_keeps_context_without_stealing_relations_or_numbers() {
    for sql in [
        "SELECT u.na",
        "SELECT u.na FROM users u",
        "SELECT SUM(u.am",
        "SELECT * FROM users u WHERE u.na",
        "SELECT \"客户\".名",
    ] {
        let offset = sql
            .find(" FROM")
            .filter(|_| sql.starts_with("SELECT u.na FROM"))
            .unwrap_or(sql.len());
        let position = position_at_end(&sql[..offset]);
        let mut parser = SqlParser::new();
        let tree = parser.parse_syntax_tree(sql).unwrap();
        let node = parser.get_node_at_position(&tree, position).unwrap();
        assert_eq!(
            parser.analyze_completion_context(node, sql, position),
            CompletionContext::TableColumn,
            "{sql}"
        );
        assert_eq!(
            SqlParser::completion_context_from_text(sql, position),
            CompletionContext::TableColumn,
            "{sql}"
        );
    }
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM public.us"),
        CompletionContext::FromClause
    );
    for sql in [
        "SELECT 1.23",
        "SELECT 'u.na",
        "SELECT 1 -- u.na",
        "SELECT 1 /* u.na",
    ] {
        assert_ne!(
            analyzed_context_at_end(sql),
            CompletionContext::TableColumn,
            "{sql}"
        );
    }
}

#[test]
fn completion_context_ignores_keywords_inside_comments_and_literals() {
    let parser = SqlParser::new();

    let from_sql = "SELECT * -- WHERE hidden\nFROM ";
    assert_eq!(
        parser.analyze_completion_context_fallback(from_sql, position_at_end(from_sql)),
        CompletionContext::FromClause
    );

    let where_sql =
        "SELECT '-- FROM hidden' AS note FROM users WHERE name = 'ORDER BY hidden' AND ";
    assert_eq!(
        parser.analyze_completion_context_fallback(where_sql, position_at_end(where_sql)),
        CompletionContext::WhereClause
    );
}

#[test]
fn completion_context_prefers_completed_clause_keywords_over_ast_noise() {
    assert_eq!(
        analyzed_context_at_end("SELECT"),
        CompletionContext::SelectClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT owner "),
        CompletionContext::SelectContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * "),
        CompletionContext::SelectContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT owner F"),
        CompletionContext::SelectContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT owner, "),
        CompletionContext::SelectClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * from"),
        CompletionContext::FromClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where "),
        CompletionContext::WhereClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where owner = "),
        CompletionContext::ExpressionValueClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where owner = N"),
        CompletionContext::ExpressionValueClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where owner = 'app' "),
        CompletionContext::PredicateContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where owner = 'app' O"),
        CompletionContext::PredicateContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where owner = 'app' AND "),
        CompletionContext::WhereClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where owner BETWEEN 1 AND "),
        CompletionContext::ExpressionValueClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where owner BETWEEN 1 "),
        CompletionContext::PredicateContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where owner BETWEEN 1 A"),
        CompletionContext::PredicateContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where owner BETWEEN 1 AND 5 "),
        CompletionContext::PredicateContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where owner IN ("),
        CompletionContext::ExpressionValueClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where owner IN ('app', "),
        CompletionContext::ExpressionValueClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where owner IN ('app', N"),
        CompletionContext::ExpressionValueClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users where owner IN ('app') "),
        CompletionContext::PredicateContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE WHEN "),
        CompletionContext::WhereClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE WHEN owner = "),
        CompletionContext::ExpressionValueClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE WHEN owner = 'app' "),
        CompletionContext::PredicateContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE WHEN owner = 'app' T"),
        CompletionContext::PredicateContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE WHEN owner BETWEEN 1 "),
        CompletionContext::PredicateContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE owner WHEN "),
        CompletionContext::CaseResultClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE owner WHEN N"),
        CompletionContext::CaseResultClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE owner WHEN 'app' "),
        CompletionContext::CaseWhenValueContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE owner WHEN 'app' T"),
        CompletionContext::CaseWhenValueContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE owner WHEN 'app' THEN 'yes' WHEN "),
        CompletionContext::CaseResultClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE WHEN owner = 'app' THEN "),
        CompletionContext::CaseResultClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE WHEN owner = 'app' THEN N"),
        CompletionContext::CaseResultClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE WHEN owner = 'app' THEN 'yes' "),
        CompletionContext::CaseContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE WHEN owner = 'app' THEN 'end' "),
        CompletionContext::CaseContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE WHEN owner = 'app' THEN 'yes' E"),
        CompletionContext::CaseContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE WHEN owner = 'app' THEN 'yes' ELSE "),
        CompletionContext::CaseResultClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE WHEN owner = 'app' THEN 'yes' ELSE 'no' "),
        CompletionContext::CaseContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT CASE WHEN owner = 'app' THEN 'yes' END "),
        CompletionContext::SelectContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users "),
        CompletionContext::FromContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users J"),
        CompletionContext::FromContinuationClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users, "),
        CompletionContext::FromClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users JOIN orders "),
        CompletionContext::JoinConditionClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users JOIN orders O"),
        CompletionContext::JoinConditionClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users ORDER BY"),
        CompletionContext::OrderByClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT name, count(*) FROM users GROUP BY"),
        CompletionContext::GroupByClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT name, count(*) FROM users GROUP BY name HAVING"),
        CompletionContext::HavingClause
    );
    assert_eq!(
        analyzed_context_at_end("INSERT INTO"),
        CompletionContext::FromClause
    );
    assert_eq!(
        analyzed_context_at_end("UPDATE"),
        CompletionContext::FromClause
    );
    assert_eq!(
        analyzed_context_at_end("DELETE FROM"),
        CompletionContext::FromClause
    );
    assert_eq!(
        analyzed_context_at_end("TRUNCATE TABLE"),
        CompletionContext::FromClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users u JOIN orders o ON"),
        CompletionContext::WhereClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users u JOIN orders o USING ("),
        CompletionContext::UsingClause
    );
    assert_eq!(
        analyzed_context_at_end("SELECT * FROM users u JOIN orders o USING (id, "),
        CompletionContext::UsingClause
    );
    assert_eq!(
        analyzed_context_at_end("CREATE INDEX users_email_idx ON"),
        CompletionContext::FromClause
    );
}

#[test]
fn completion_context_ignores_keywords_inside_dollar_quotes() {
    let parser = SqlParser::new();

    let sql = r#"
        CREATE FUNCTION app.hidden_lookup()
        RETURNS integer
        LANGUAGE SQL
        AS $$ SELECT id FROM hidden.internal WHERE active = true; $$;
        SELECT
    "#;

    assert_eq!(
        parser.analyze_completion_context_fallback(sql, position_at_end(sql)),
        CompletionContext::SelectClause
    );
}

#[test]
fn completion_context_respects_keyword_boundaries() {
    let parser = SqlParser::new();
    let sql = "SELECT * FROM order_items ";

    assert_eq!(
        parser.analyze_completion_context_fallback(sql, position_at_end(sql)),
        CompletionContext::FromContinuationClause
    );
}

#[test]
fn fallback_completion_context_uses_latest_grouping_clause() {
    let parser = SqlParser::new();

    let having_sql = "SELECT user_id, count(*) FROM orders GROUP BY user_id HAVING count(*) > ";
    assert_eq!(
        parser.analyze_completion_context_fallback(having_sql, position_at_end(having_sql)),
        CompletionContext::ExpressionValueClause
    );

    let having_continuation_sql =
        "SELECT user_id, count(*) FROM orders GROUP BY user_id HAVING count(*) > 1 ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            having_continuation_sql,
            position_at_end(having_continuation_sql)
        ),
        CompletionContext::PredicateContinuationClause
    );

    let order_sql =
        "SELECT user_id, count(*) FROM orders GROUP BY user_id HAVING count(*) > 1 ORDER BY ";
    assert_eq!(
        parser.analyze_completion_context_fallback(order_sql, position_at_end(order_sql)),
        CompletionContext::OrderByClause
    );

    let group_sql = "SELECT user_id, count(*) FROM orders GROUP BY ";
    assert_eq!(
        parser.analyze_completion_context_fallback(group_sql, position_at_end(group_sql)),
        CompletionContext::GroupByClause
    );

    let group_continuation_sql = "SELECT user_id, count(*) FROM orders GROUP BY user_id ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            group_continuation_sql,
            position_at_end(group_continuation_sql)
        ),
        CompletionContext::GroupByContinuationClause
    );

    let group_continuation_prefix_sql = "SELECT user_id, count(*) FROM orders GROUP BY user_id H";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            group_continuation_prefix_sql,
            position_at_end(group_continuation_prefix_sql)
        ),
        CompletionContext::GroupByContinuationClause
    );

    let group_after_comma_sql = "SELECT user_id, count(*) FROM orders GROUP BY user_id, ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            group_after_comma_sql,
            position_at_end(group_after_comma_sql)
        ),
        CompletionContext::GroupByClause
    );

    let order_direction_sql = "SELECT * FROM users ORDER BY name ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            order_direction_sql,
            position_at_end(order_direction_sql)
        ),
        CompletionContext::OrderDirectionClause
    );

    let order_direction_prefix_sql = "SELECT * FROM users ORDER BY name D";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            order_direction_prefix_sql,
            position_at_end(order_direction_prefix_sql)
        ),
        CompletionContext::OrderDirectionClause
    );

    let order_nulls_prefix_sql = "SELECT * FROM users ORDER BY name DESC N";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            order_nulls_prefix_sql,
            position_at_end(order_nulls_prefix_sql)
        ),
        CompletionContext::OrderDirectionClause
    );

    let order_after_direction_sql = "SELECT * FROM users ORDER BY name DESC ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            order_after_direction_sql,
            position_at_end(order_after_direction_sql)
        ),
        CompletionContext::OrderDirectionClause
    );

    let order_after_nulls_sql = "SELECT * FROM users ORDER BY name DESC NULLS FIRST ";
    assert_eq!(
        parser.analyze_completion_context_fallback(
            order_after_nulls_sql,
            position_at_end(order_after_nulls_sql)
        ),
        CompletionContext::OrderDirectionClause
    );
}

#[test]
fn text_fallback_distinguishes_searched_case_join_and_ddl_condition_sites() {
    for (sql, expected) in [
        ("SELECT CASE WHEN ", CompletionContext::WhereClause),
        (
            "SELECT CASE u.id WHEN ",
            CompletionContext::CaseResultClause,
        ),
        (
            "SELECT * FROM users u JOIN orders o ON ",
            CompletionContext::WhereClause,
        ),
        ("CREATE INDEX idx ON ", CompletionContext::FromClause),
    ] {
        assert_eq!(
            SqlParser::completion_context_from_text(sql, position_at_end(sql)),
            expected,
            "{sql}"
        );
    }
}

#[test]
fn closed_subquery_clauses_do_not_override_active_outer_completion_context() {
    for (marked, expected) in [
        (
            "WITH c AS (SELECT id FROM users WHERE id>0) SELECT na| FROM c",
            CompletionContext::SelectClause,
        ),
        (
            "WITH c AS (SELECT id FROM users WHERE id>0) SELECT * FROM us|",
            CompletionContext::FromClause,
        ),
        (
            "SELECT COALESCE((SELECT name FROM users WHERE id=1), na|) FROM users",
            CompletionContext::SelectClause,
        ),
        (
            "SELECT * FROM (SELECT id FROM users WHERE id>0) d WHERE d.na|",
            CompletionContext::TableColumn,
        ),
    ] {
        let byte = marked.find('|').unwrap();
        let sql = marked.replace('|', "");
        let point = crate::position::byte_position_at_end(&sql[..byte]);
        let mut parser = SqlParser::new();
        let tree = parser.parse_syntax_tree(&sql).unwrap();
        let node = parser.get_node_at_position(&tree, point).unwrap();
        assert_eq!(
            parser.analyze_completion_context(node, &sql, point),
            expected,
            "{marked}"
        );
        assert_eq!(
            SqlParser::completion_context_from_text(&sql, point),
            expected,
            "fallback {marked}"
        );
    }
}
