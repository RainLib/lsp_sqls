use super::*;
#[test]
fn joined_tables_natural_join_owns_implicit_condition() {
    for sql in [
        "SELECT id FROM users NATURAL JOIN orders;",
        "SELECT id FROM users NATURAL LEFT OUTER JOIN orders;",
        "SELECT id FROM users NATURAL RIGHT JOIN orders;",
        "SELECT id FROM users NATURAL FULL OUTER JOIN orders;",
    ] {
        let tree = SqlParser::new().parse_syntax_tree(sql).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "{sql}: {}",
            tree.root_node().to_sexp()
        );
    }
}
#[test]
fn joined_tables_parenthesized_inputs_and_comma_chains_are_real_nodes() {
    for sql in ["SELECT u.name FROM (users u JOIN orders o ON u.id=o.customer_id);","SELECT o.total FROM users u JOIN (orders o JOIN audit_events a ON true) ON u.id=o.customer_id;","SELECT g.total FROM ((users u JOIN orders o ON u.id=o.customer_id)) g;","SELECT * FROM users u JOIN orders o ON u.id=o.customer_id, audit_events a JOIN users v ON true;"] {
        let mut parser=SqlParser::new();let tree=parser.parse_syntax_tree(sql).unwrap();assert!(!tree.root_node().has_error(),"{sql}: {}",tree.root_node().to_sexp());
        if sql.contains("(users") {assert!(parser.relation_scopes_at_position(&tree,sql,tower_lsp::lsp_types::Position::new(0,10))[0].joined.is_some(),"{} {:?}",tree.root_node().to_sexp(),parser.relation_scopes_at_position(&tree,sql,tower_lsp::lsp_types::Position::new(0,10)));}
    }
}
#[test]
fn joined_tables_natural_does_not_accept_explicit_on_or_using() {
    for sql in [
        "SELECT * FROM users NATURAL JOIN orders ON users.id=orders.id;",
        "SELECT * FROM users NATURAL JOIN orders USING(id);",
        "SELECT * FROM users JOIN orders;",
    ] {
        let result = SqlParser::new().parse(sql);
        assert!(
            !result.diagnostics.is_empty(),
            "{sql}: {}",
            result.tree.unwrap().root_node().to_sexp()
        );
    }
}

#[test]
fn joined_tables_unfinished_member_still_binds_real_from_layout() {
    let sql = "SELECT g. FROM (users u NATURAL JOIN orders o) g";
    let mut parser = SqlParser::new();
    let tree = parser
        .parse_completion_tree(sql, tower_lsp::lsp_types::Position::new(0, 9))
        .unwrap();
    let scopes =
        parser.relation_scopes_at_position(&tree, sql, tower_lsp::lsp_types::Position::new(0, 9));
    assert!(
        scopes[0].joined.is_some(),
        "{} {scopes:?}",
        tree.root_node().to_sexp()
    );
    assert_eq!(parser.syntax_parse_count, 1);
    assert!(
        parser.parse(sql).tree.unwrap().root_node().has_error(),
        "completion recovery cannot hide validation errors"
    );
}

#[test]
fn joined_tables_completion_recovery_never_touches_opaque_operands() {
    for sql in [
        "SELECT 'g. FROM (a JOIN b ON true)'",
        "SELECT 1 /*g. FROM (a JOIN b ON true)*/",
    ] {
        let mut parser = SqlParser::new();
        let at = sql.find("g.").unwrap() + 2;
        parser
            .parse_completion_tree(sql, crate::position::byte_position_at_end(&sql[..at]))
            .unwrap();
        assert_eq!(
            parser.source, sql,
            "only canonical parsing may remember opaque input"
        );
        assert_eq!(parser.syntax_parse_count, 1);
    }
}
