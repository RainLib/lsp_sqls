use super::*;
use tower_lsp::lsp_types::{Diagnostic, NumberOrString};
fn schema() -> Schema {
    let mut s = completion_schema_with_columns();
    s.tables.push(Table {
        name: "third".into(),
        columns: vec![Column {
            name: "id".into(),
            data_type: "bigint".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    s
}
fn warnings(sql: &str, dialect: &str) -> Vec<Diagnostic> {
    sql_inspection_diagnostics(sql, Some(&schema()), dialect)
        .into_iter()
        .filter(|d| d.code == Some(NumberOrString::String("OXIDE004".into())))
        .collect()
}
fn check(sql: &str, n: usize) {
    let ds = warnings(sql, "postgres");
    if ds.len() != n {
        let tree = crate::SqlParser::new().parse_syntax_tree(sql).unwrap();
        panic!("{sql}: {ds:?}: {}", tree.root_node().to_sexp());
    }
}
#[test]
fn ambiguity_scopes_only_field_references_are_candidates() {
    for sql in [
        "SELECT o.id AS id FROM orders o JOIN customers c ON o.customer_id=c.id;",
        "SELECT 1 AS id FROM orders o JOIN customers c ON o.customer_id=c.id;",
        "SELECT count(o.id) AS id FROM orders o JOIN customers c ON o.customer_id=c.id;",
    ] {
        check(sql, 0);
    }
    check(
        "SELECT id FROM orders o JOIN customers c ON o.customer_id=c.id;",
        1,
    );
}
#[test]
fn ambiguity_scopes_subqueries_do_not_share_sibling_rows() {
    for sql in ["SELECT id,(SELECT id FROM customers) AS value FROM orders;","SELECT id FROM orders WHERE EXISTS(SELECT id FROM customers WHERE customers.id=orders.customer_id);","SELECT o.id FROM orders o WHERE EXISTS(SELECT id FROM customers c);"]{check(sql,0);}
}
#[test]
fn ambiguity_scopes_self_join_counts_distinct_occurrences() {
    check("SELECT id FROM orders a JOIN orders b ON a.id=b.id;", 1);
    check("SELECT a.id FROM orders a JOIN orders b ON a.id=b.id;", 0);
}
#[test]
fn ambiguity_scopes_ctes_and_derived_outputs_are_bound_locally() {
    check(
        "WITH q AS(SELECT id FROM orders) SELECT id FROM q JOIN customers c ON q.id=c.id;",
        1,
    );
    check(
        "SELECT id FROM (SELECT id FROM orders) q JOIN customers c ON q.id=c.id;",
        1,
    );
    check("WITH orders AS(SELECT name FROM customers) SELECT id FROM orders q JOIN app.orders p ON q.name='x';",0);
}
#[test]
fn ambiguity_scopes_using_merges_only_its_own_inputs() {
    check("SELECT id FROM orders JOIN customers USING(id);", 0);
    check(
        "SELECT id FROM orders JOIN customers USING(id) JOIN third t ON t.id=orders.id;",
        1,
    );
    check(
        "SELECT id FROM orders JOIN customers USING(id) JOIN third USING(id);",
        0,
    );
}
#[test]
fn ambiguity_scopes_order_output_alias_does_not_bind_input() {
    check(
        "SELECT o.id AS id FROM orders o JOIN customers c ON c.id=o.customer_id ORDER BY id;",
        0,
    );
    check(
        "SELECT o.id AS id FROM orders o JOIN customers c ON c.id=o.customer_id GROUP BY id;",
        1,
    );
}
#[test]
fn ambiguity_scopes_quoted_fields_keep_complete_utf16_ranges() {
    let sql = "/*😀*/\r\nSELECT \"id\" FROM orders o JOIN customers c ON o.customer_id=c.id;";
    let d = warnings(sql, "postgres");
    assert_eq!(d.len(), 1);
    let at = sql.find("\"id\"").unwrap();
    assert_eq!(d[0].range, range_for_offsets(sql, at, at + 4));
    check(
        "SELECT o /*🧭*/ . id FROM orders o JOIN customers c ON o.customer_id=c.id;",
        0,
    );
}
#[test]
fn ambiguity_scopes_correlated_names_obey_nearest_scope() {
    check("SELECT o.id,(SELECT id FROM third t) AS value FROM orders o JOIN customers c ON c.id=o.customer_id;",0);
    check(
        "SELECT o.id,(SELECT id) AS value FROM orders o JOIN customers c ON c.id=o.customer_id;",
        1,
    );
    check("SELECT o.id FROM orders o JOIN customers c ON c.id=o.customer_id JOIN (SELECT id) q ON true;",0);
}
#[test]
fn ambiguity_scopes_union_branches_keep_their_own_rows() {
    check(
        "SELECT id FROM orders UNION ALL SELECT id FROM customers;",
        0,
    );
    check("SELECT id FROM orders o JOIN customers c ON o.customer_id=c.id UNION ALL SELECT id FROM third;",1);
}
#[test]
fn ambiguity_scopes_assignment_targets_and_non_sql_names_are_not_reads() {
    let sql = "UPDATE orders o JOIN customers c ON o.customer_id=c.id SET o.id=id;";
    assert_eq!(warnings(sql, "mysql").len(), 1);
    check(
        "UPDATE orders SET id=(SELECT id FROM customers WHERE id=1);",
        0,
    );
}
#[test]
fn ambiguity_scopes_nested_directive_does_not_suppress_outer_field() {
    check("SELECT id,(SELECT c.id FROM customers c /* noinspection OXIDE004 */) AS value FROM orders o JOIN customers c ON c.id=o.customer_id;",1);
    check("/* noinspection OXIDE004 */ SELECT id FROM orders o JOIN customers c ON c.id=o.customer_id;",0);
}
#[test]
fn ambiguity_scopes_unresolved_relations_do_not_fabricate_outer_ambiguity() {
    check("SELECT o.id,(SELECT id FROM not_loaded x) AS value FROM orders o JOIN customers c ON c.id=o.customer_id;",0);
}

#[test]
fn ambiguity_scopes_unavailable_local_shape_cannot_bind_physical_shadow() {
    let source=format!("/*{}*/ WITH orders AS(SELECT name FROM customers) SELECT id FROM orders q JOIN app.orders p ON q.name='x';","padding ".repeat(70000));
    check(&source, 0);
}
#[test]
fn ambiguity_scopes_closed_sibling_cte_does_not_hide_real_outer_rows() {
    check("SELECT id FROM orders o JOIN customers c ON o.customer_id=c.id WHERE EXISTS(WITH orders AS(SELECT name FROM customers) SELECT 1 FROM orders);",1);
}

#[test]
fn ambiguity_scopes_qualification_uses_byte_position_after_nested_unicode() {
    let sql = format!(
        "SELECT (SELECT '{}') AS label, id FROM orders o JOIN customers c ON o.customer_id=c.id;",
        "😀".repeat(64)
    );
    let at = sql.find(", id").unwrap() + 4;
    let pos = lsp_position_at_end(&sql[..at]);
    let uri = Url::parse("oxide://ambiguity/unicode.sql").unwrap();
    let actions = qualify_identifier_actions(
        &sql,
        &uri,
        Range::new(pos, pos),
        Some(&schema()),
        "postgres",
    );
    let replacements: Vec<_> = actions
        .iter()
        .map(|a| code_action_replacement(a, &uri))
        .collect();
    assert!(
        replacements.contains(&"o.id".into()) && replacements.contains(&"c.id".into()),
        "{replacements:?}"
    );
}
