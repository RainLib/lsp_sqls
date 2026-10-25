use super::*;

fn marked(input: &str) -> (String, Position) {
    let at = input.find("<cursor>").unwrap();
    let sql = input.replace("<cursor>", "");
    let position = SqlParser::lsp_position_to_byte_position(
        &sql,
        crate::position::lsp_position_at_end(&input[..at]),
    );
    (sql, position)
}

#[test]
fn shared_scope_mask_preserves_utf8_crlf_and_identifier_offsets() {
    let (sql,position)=marked("SELECT \"外😀\".<cursor>\r\nFROM users AS \"外😀\" JOIN (SELECT '雪😀' AS name FROM hidden h) d ON true");
    let scope = SqlParser::completion_scope_analysis(&sql, position);
    assert_eq!(scope.source.len(), sql.len());
    assert_eq!(scope.searchable.len(), sql.len());
    assert!(scope.source.contains("\"外😀\""));
    assert!(!scope.source.contains("hidden"));
    assert_eq!(scope.source.find("users"), sql.find("users"));
    assert_eq!(scope.source.find("\r\n"), sql.find("\r\n"));
    assert_eq!(scope.searchable, SqlParser::mask_sql_noise(&scope.source));
}

#[test]
fn scope_aliases_do_not_import_nested_sibling_relations() {
    let (sql,position)=marked("SELECT u.<cursor> FROM users u JOIN (SELECT h.id FROM hidden h) d ON true WHERE EXISTS(SELECT 1 FROM secrets s)");
    let aliases = SqlParser::relation_aliases_at_position(&sql, position);
    assert!(aliases
        .iter()
        .any(|alias| alias.name == "u" && alias.relation == "users"));
    assert!(!aliases
        .iter()
        .any(|alias| matches!(alias.name.as_str(), "h" | "s")));
}

#[test]
fn correlated_scope_imports_only_requested_outer_alias() {
    let (sql,position)=marked("SELECT * FROM users u JOIN orders o ON o.user_id=u.id WHERE EXISTS(SELECT 1 FROM audit a WHERE a.actor_id=u.<cursor>)");
    let aliases = SqlParser::relation_aliases_at_position(&sql, position);
    assert!(aliases
        .iter()
        .any(|alias| alias.name == "u" && alias.relation == "users"));
    assert!(aliases
        .iter()
        .any(|alias| alias.name == "a" && alias.relation == "audit"));
    assert!(!aliases.iter().any(|alias| alias.name == "o"));
}

#[test]
fn quoted_literals_and_comments_cannot_end_the_statement_scope() {
    let (sql,position)=marked("SELECT 'FROM ghost g;'; /* FROM phantom p; */ SELECT u.<cursor> FROM users u WHERE u.name='雪😀; FROM fake f'");
    let aliases = SqlParser::relation_aliases_at_position(&sql, position);
    assert_eq!(aliases.len(), 1);
    assert_eq!(aliases[0].name, "u");
    assert_eq!(aliases[0].relation, "users");
}

#[test]
fn shared_scope_keeps_function_alias_after_hidden_arguments() {
    let (sql,position)=marked("SELECT j.<cursor> FROM users u,JSON_TABLE(u.payload,'$[*]' COLUMNS(id INT PATH '$.id',nested_json JSON PATH '$.x')) j");
    let scope = SqlParser::completion_scope_analysis(&sql, position);
    assert!(!scope.source.contains("COLUMNS"));
    assert_eq!(scope.searchable, SqlParser::mask_sql_noise(&scope.source));
    let aliases = SqlParser::relation_aliases_at_position(&sql, position);
    assert!(aliases
        .iter()
        .any(|alias| alias.name == "j" && alias.relation.eq_ignore_ascii_case("JSON_TABLE")));
}

#[test]
fn following_soft_statement_does_not_replace_current_alias() {
    let (sql, position) =
        marked("SELECT u.<cursor> FROM users u\r\n\r\nSELECT o.total FROM orders o");
    let aliases = SqlParser::relation_aliases_at_position(&sql, position);
    assert_eq!(aliases.len(), 1);
    assert_eq!(aliases[0].name, "u");
}

#[test]
fn function_parentheses_are_not_queries_because_of_closed_subqueries_or_modes() {
    for sql in [
        "SELECT COALESCE((SELECT name FROM users), ",
        "SELECT MATCH(name) AGAINST ('oxide' WITH QUERY EXPANSION ",
        "SELECT fn(表SELECT, value ",
    ] {
        let mask = SqlParser::mask_sql_noise(sql);
        assert_eq!(
            SqlParser::innermost_query_scope_start(&mask).1,
            None,
            "{sql}"
        );
    }
    let sql = "SELECT COALESCE((SELECT name FROM users WHERE ";
    let mask = SqlParser::mask_sql_noise(sql);
    assert_eq!(
        SqlParser::innermost_query_scope_start(&mask).1,
        sql.find("(SELECT")
    );
}

fn correlation_scopes(marked_sql: &str) -> Vec<crate::parser::RelationScope> {
    let (sql, position) = marked(marked_sql);
    let mut parser = SqlParser::new();
    let tree = parser.parse_syntax_tree(&sql).unwrap();
    parser.relation_scopes_at_position(&tree, &sql, position)
}
#[test]
fn correlation_scope_chain_preserves_inner_and_outer_aliases_separately() {
    let scopes = correlation_scopes(
        "/* 🧭 */\r\nSELECT * FROM orders o WHERE EXISTS(SELECT <cursor> FROM users u)",
    );
    assert_eq!(scopes.len(), 2);
    assert_eq!(scopes[0].relations, ["users"]);
    assert_eq!(scopes[1].relations, ["orders"]);
    assert_eq!(scopes[0].aliases[0].name, "u");
    assert_eq!(scopes[1].aliases[0].name, "o");
    assert!(scopes[0].start > scopes[1].start);
}
#[test]
fn derived_and_cte_edges_skip_their_main_query_rows() {
    for sql in [
        "SELECT * FROM orders o JOIN (SELECT <cursor>) d ON TRUE",
        "WITH c AS(SELECT <cursor>) SELECT * FROM orders o",
    ] {
        let scopes = correlation_scopes(sql);
        assert_eq!(scopes.len(), 1, "{sql}");
        assert!(scopes[0].relations.is_empty());
    }
    let scopes = correlation_scopes(
        "SELECT * FROM orders o WHERE EXISTS(SELECT 1 FROM (SELECT <cursor>) d)",
    );
    assert_eq!(scopes.len(), 2);
    assert_eq!(scopes[1].relations, ["orders"]);
}
#[test]
fn lateral_correlation_scope_excludes_its_own_alias_and_right_hand_rows() {
    let scopes = correlation_scopes(
        "SELECT * FROM users u CROSS JOIN LATERAL(SELECT <cursor>) d CROSS JOIN orders o",
    );
    assert_eq!(scopes.len(), 2);
    assert_eq!(scopes[1].relations, ["users"]);
    assert_eq!(scopes[1].aliases.len(), 1);
    assert_eq!(scopes[1].aliases[0].name, "u");
}
#[test]
fn incomplete_correlation_edges_keep_explicit_derived_boundaries() {
    let blocked = correlation_scopes("SELECT * FROM orders o JOIN (SELECT o.<cursor>");
    assert_eq!(blocked.len(), 1);
    let lateral = correlation_scopes("SELECT * FROM orders o CROSS JOIN LATERAL(SELECT o.<cursor>");
    assert_eq!(lateral.len(), 2);
    assert_eq!(lateral[1].relations, ["orders"]);
}
#[test]
fn correlation_scope_depth_is_bounded_and_functions_are_not_query_levels() {
    let scopes = correlation_scopes(
        "SELECT * FROM orders o WHERE EXISTS(SELECT coalesce((SELECT <cursor>),0))",
    );
    assert_eq!(scopes.len(), 3);
    assert_eq!(scopes[2].relations, ["orders"]);
    let mut sql = "SELECT <cursor>".to_string();
    for _ in 0..40 {
        sql = format!("SELECT ({sql})");
    }
    assert!(correlation_scopes(&sql).is_empty());
}

#[test]
fn incomplete_comma_source_is_isolated_but_scalar_argument_is_correlated() {
    let derived = correlation_scopes("SELECT * FROM orders o, (SELECT o.<cursor>");
    assert_eq!(derived.len(), 1);
    let scalar = correlation_scopes("SELECT * FROM orders o WHERE coalesce(1,(SELECT o.<cursor>");
    assert_eq!(scalar.len(), 2);
    assert_eq!(scalar[1].relations, ["orders"]);
}
#[test]
fn lateral_groups_visibility_ignores_join_parentheses_but_keeps_query_boundaries() {
    let cte = "SELECT (WITH ghost AS(SELECT 1) SELECT * FROM ghost), 1";
    assert!(!SqlParser::query_source_visible(
        cte,
        cte.find("WITH").unwrap(),
        cte.len() - 1
    ));
    let sql = "SELECT d.value FROM (orders o CROSS APPLY(SELECT 1 AS value) d)";
    let at = sql.find("APPLY").unwrap() + 5;
    assert!(SqlParser::query_source_visible(sql, at, 8));
    let sql = "SELECT 1 FROM (SELECT d.value FROM (orders o CROSS APPLY(SELECT 1 AS value) d)) q";
    let at = sql.find("APPLY").unwrap() + 5;
    assert!(!SqlParser::query_source_visible(sql, at, 8));
    assert!(SqlParser::query_source_visible(
        sql,
        at,
        sql.find("d.value").unwrap()
    ));
}
