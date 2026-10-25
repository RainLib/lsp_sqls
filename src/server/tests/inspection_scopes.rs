use super::*;
use crate::SqlParser;
use tower_lsp::lsp_types::{Diagnostic, NumberOrString};
fn items(sql: &str, code: &str) -> Vec<Diagnostic> {
    sql_inspection_diagnostics(sql, None, "postgres")
        .into_iter()
        .filter(|d| d.code == Some(NumberOrString::String(code.into())))
        .collect()
}
#[test]
fn inspection_scopes_inner_where_does_not_protect_outer_mutation() {
    for sql in [
        "UPDATE users SET id=(SELECT id FROM orders WHERE id=1);",
        "UPDATE users SET id=CASE WHEN EXISTS(SELECT 1 FROM orders WHERE id=1) THEN 1 ELSE 2 END;",
        "UPDATE users SET id=1 FROM (SELECT id FROM orders WHERE id=1) q;",
        "WITH q AS(SELECT id FROM orders WHERE id=1) UPDATE users SET id=(SELECT id FROM q);",
        "WITH q AS(SELECT id FROM orders) DELETE FROM users;",
    ] {
        let d = items(sql, "OXIDE001");
        let tree = SqlParser::new().parse_syntax_tree(sql).unwrap();
        assert_eq!(d.len(), 1, "{sql}: {}", tree.root_node().to_sexp());
    }
}
#[test]
fn inspection_scopes_actual_outer_where_keeps_mutations_filtered() {
    for sql in [
        "UPDATE users SET id=(SELECT id FROM orders WHERE id=1) WHERE id=2;",
        "WITH q AS(SELECT id FROM orders WHERE id=1) UPDATE users SET id=(SELECT id FROM q) WHERE id=2;",
        "WITH q AS(SELECT id FROM orders) DELETE FROM users WHERE id IN(SELECT id FROM q);",
    ] {assert!(items(sql,"OXIDE001").is_empty(),"{sql}");}
}
#[test]
fn inspection_scopes_cte_body_and_procedure_mutations_are_independent() {
    for (dialect, sql, expected) in [
        (
            "postgres",
            "WITH changed AS(DELETE FROM users RETURNING id) SELECT id FROM changed;",
            1,
        ),
        (
            "postgres",
            "WITH changed AS(DELETE FROM users WHERE id=1 RETURNING id) UPDATE users SET id=2;",
            1,
        ),
        (
            "sqlserver",
            "CREATE PROC app.p AS BEGIN UPDATE users SET id=1; DELETE FROM users WHERE id=2; END;",
            1,
        ),
    ] {
        let d = sql_inspection_diagnostics(sql, None, dialect);
        let n = diagnostic_codes(&d)
            .iter()
            .filter(|c| c.as_str() == "OXIDE001")
            .count();
        let tree = SqlParser::new().parse_syntax_tree(sql).unwrap();
        assert_eq!(n, expected, "{sql}: {}", tree.root_node().to_sexp());
    }
}
#[test]
fn inspection_scopes_inner_join_predicate_does_not_protect_outer_join() {
    for sql in [
        "SELECT a.id FROM a JOIN (SELECT b.id FROM b JOIN c ON b.id=c.id) q WHERE a.id>0;",
        "SELECT a.id FROM a JOIN b WHERE EXISTS(SELECT 1 FROM c JOIN d ON c.id=d.id);",
        "SELECT a.id FROM a JOIN b\nWHERE a.id>0;",
        "SELECT a.id FROM a JOIN b\tWHERE a.id>0;",
    ] {
        assert_eq!(items(sql, "OXIDE002").len(), 1, "{sql}");
    }
}
#[test]
fn inspection_scopes_every_missing_join_has_its_own_diagnostic() {
    let sql = "SELECT a.id FROM a JOIN b JOIN c WHERE a.id>0;";
    assert_eq!(items(sql, "OXIDE002").len(), 2);
    for sql in [
        "SELECT a.id FROM a CROSS JOIN b JOIN c WHERE a.id>0;",
        "SELECT a.id FROM a NATURAL JOIN b JOIN c WHERE a.id>0;",
    ] {
        assert_eq!(items(sql, "OXIDE002").len(), 1, "{sql}");
    }
}
#[test]
fn inspection_scopes_quickfix_preserves_quoted_relation_and_newline_boundary() {
    let uri = Url::parse("oxide://inspection/scopes.sql").unwrap();
    for sql in [
        "SELECT 1 FROM a JOIN \"Order Table\"\nWHERE 1=1;",
        "SELECT 1 FROM a JOIN \"Order Table\" AS \"Right Alias\"\tWHERE 1=1;",
        "SELECT 1 FROM a JOIN (SELECT 1 AS id) q\nWHERE 1=1;",
    ] {
        let d = items(sql, "OXIDE002");
        assert_eq!(d.len(), 1, "{sql}");
        let actions = add_join_condition_actions(sql, &uri, &d[0], None, "postgres");
        assert!(!actions.is_empty(), "{sql}");
        let edit = &code_action_edits(&actions[0], &uri)[0];
        let at = position_to_byte_offset(sql, edit.range.start);
        assert_eq!(
            at,
            sql.find('\n').or_else(|| sql.find('\t')).unwrap(),
            "{sql}: {edit:?}"
        );
        let mut accepted = sql.to_owned();
        accepted.insert_str(at, &edit.new_text);
        assert!(items(&accepted, "OXIDE002").is_empty(), "{accepted}");
        let parsed = SqlParser::new().parse(&accepted);
        assert!(
            parsed.diagnostics.is_empty(),
            "{accepted}: {:?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn inspection_scopes_nested_comment_cannot_suppress_an_enclosing_mutation() {
    for sql in [
        "WITH q AS(SELECT id FROM orders /* noinspection OXIDE001 */) UPDATE users SET id=1;",
        "UPDATE users SET id=(SELECT id FROM orders /* noinspection OXIDE001 */);",
        "WITH changed AS(DELETE FROM users /* noinspection OXIDE001 */ RETURNING id) UPDATE users SET id=2;",
    ] {assert_eq!(items(sql,"OXIDE001").len(),1,"{sql}");}
}
#[test]
fn inspection_scopes_valid_join_conditions_and_quoted_names_stay_opaque() {
    for (dialect,sql) in [
        ("postgres","SELECT a.id FROM a JOIN b ON EXISTS(SELECT 1 FROM c JOIN d ON c.id=d.id) WHERE a.id>0;"),
        ("postgres","SELECT a.id FROM a JOIN (SELECT b.id FROM b JOIN c ON b.id=c.id) q ON a.id=q.id;"),
        ("postgres","SELECT \"JOIN\" FROM \"CROSS\";"),
        ("mysql","SELECT `JOIN` FROM `WHERE` a JOIN `ON` b USING(id);"),
        ("sqlserver","SELECT [JOIN] FROM [WHERE] a JOIN [ON] b ON a.id=b.id;"),
    ] {let d=sql_inspection_diagnostics(sql,None,dialect);assert!(!diagnostic_codes(&d).contains(&"OXIDE002".into()),"{sql}: {d:?}");}
}
#[test]
fn inspection_scopes_each_missing_join_action_targets_its_own_diagnostic() {
    let sql = "/*😀*/ SELECT a.id FROM a JOIN b JOIN c\nWHERE a.id>0;";
    let uri = Url::parse("oxide://inspection/two.sql").unwrap();
    let d = items(sql, "OXIDE002");
    assert_eq!(d.len(), 2);
    for (n, at) in [sql.find(" JOIN c").unwrap(), sql.find('\n').unwrap()]
        .into_iter()
        .enumerate()
    {
        let actions = add_join_condition_actions(sql, &uri, &d[n], None, "postgres");
        assert_eq!(actions.len(), 1);
        let edit = &code_action_edits(&actions[0], &uri)[0];
        assert_eq!(position_to_byte_offset(sql, edit.range.start), at);
        let mut accepted = sql.to_owned();
        accepted.insert_str(at, &edit.new_text);
        assert_eq!(items(&accepted, "OXIDE002").len(), 1);
    }
}
#[test]
fn inspection_scopes_nested_mutation_ranges_are_local_utf16_spans() {
    let sql = "/*😀*/ WITH q AS(DELETE FROM users RETURNING id) SELECT id FROM q;";
    let d = items(sql, "OXIDE001");
    assert_eq!(d.len(), 1);
    assert_eq!(
        d[0].range,
        range_for_offsets(
            sql,
            sql.find("DELETE").unwrap(),
            sql.find(") SELECT").unwrap()
        )
    );
    for sql in [
        "INSERT INTO users(id) VALUES(1) ON CONFLICT(id) DO UPDATE SET id=2;",
        "CREATE TABLE t (id int); SELECT 'UPDATE users SET id=1;';",
    ] {
        assert!(items(sql, "OXIDE001").is_empty(), "{sql}");
    }
}

#[test]
fn inspection_scopes_plan_only_explain_does_not_execute_mutations() {
    assert!(items("EXPLAIN UPDATE users SET id=1;", "OXIDE001").is_empty());
    assert_eq!(
        items("EXPLAIN ANALYZE UPDATE users SET id=1;", "OXIDE001").len(),
        1
    );
    assert!(items("EXPLAIN DELETE FROM users;", "OXIDE001").is_empty());
    assert_eq!(
        items("EXPLAIN ANALYZE DELETE FROM users;", "OXIDE001").len(),
        1
    );
}

#[test]
fn inspection_scopes_explain_mode_propagates_to_cte_mutations() {
    assert!(items(
        "EXPLAIN WITH q AS(DELETE FROM users RETURNING id) SELECT id FROM q;",
        "OXIDE001"
    )
    .is_empty());
    assert_eq!(
        items(
            "EXPLAIN ANALYZE WITH q AS(DELETE FROM users RETURNING id) SELECT id FROM q;",
            "OXIDE001"
        )
        .len(),
        1
    );
}

#[test]
fn inspection_scopes_unicode_nested_projection_keeps_outer_foreign_key_binding() {
    let mut schema = completion_schema_with_columns();
    schema.tables[0].constraints.push(Constraint {
        name: "orders_customer_fk".into(),
        constraint_type: "FOREIGN KEY".into(),
        columns: vec!["customer_id".into()],
        referenced_schema: Some("app".into()),
        referenced_table: Some("customers".into()),
        referenced_columns: vec!["id".into()],
        definition: None,
    });
    let sql = format!(
        "SELECT (SELECT '{}') AS label,o.id FROM app.orders o JOIN app.customers c;",
        "😀".repeat(64)
    );
    let warning = items(&sql, "OXIDE002").remove(0);
    let uri = Url::parse("oxide://inspection/unicode-scope.sql").unwrap();
    let actions = add_join_condition_actions(&sql, &uri, &warning, Some(&schema), "postgres");
    assert_eq!(
        actions[0].title,
        "Add JOIN condition via orders_customer_fk"
    );
    assert_eq!(
        code_action_replacement(&actions[0], &uri),
        " ON o.customer_id = c.id"
    );
}
