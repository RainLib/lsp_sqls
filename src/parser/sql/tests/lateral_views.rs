use super::*;
#[test]
fn hive_lateral_view_grammar_keeps_aliases_columns_and_physical_sources() {
    for sql in [
        "SELECT e.item FROM orders o LATERAL VIEW explode(o.tags) e AS item",
        "SELECT e.pos FROM orders o LATERAL VIEW OUTER posexplode(o.tags) e",
        "SELECT e.key,e.value FROM orders o LATERAL VIEW explode(o.props) e AS key,value",
        "SELECT e.item FROM users u JOIN orders o LATERAL VIEW explode(o.tags) e AS item ON u.id=o.id",
    ] {
        let mut parser=SqlParser::new();let parsed=parser.parse(sql);assert!(parsed.diagnostics.is_empty(),"{sql}: {:?}",parsed.diagnostics);
        let tree=parsed.tree.unwrap();let views=SqlParser::lateral_views(&tree,sql);assert_eq!(views.len(),1,"{sql}");assert_eq!(views[0].alias,"e");
        let references=parser.extract_referenced_tables(&tree,sql);assert!(references.contains(&"orders".into()));assert!(!references.iter().any(|r|r=="explode"||r=="posexplode"),"{references:?}");
        let at=sql.find("e.").unwrap()+2;let scope=parser.joined_source_scope(&tree,sql,crate::position::byte_position_at_end(&sql[..at]),0).unwrap();assert!(scope.aliases.iter().any(|a|a.name=="e"));
    }
}
#[test]
fn hive_lateral_view_arguments_see_preceding_rows_and_never_their_own_output() {
    let sql="SELECT * FROM orders o LATERAL VIEW explode(o.grid) a AS nested_col LATERAL VIEW explode(a.nested_col) b AS item";
    let mut parser = SqlParser::new();
    let tree = parser.parse(sql).tree.unwrap();
    let views = SqlParser::lateral_views(&tree, sql);
    assert_eq!(views.len(), 2);
    for (index, view) in views.iter().enumerate() {
        let at = view.arguments[0].start + 2;
        let scope = parser
            .joined_source_scope(
                &tree,
                sql,
                crate::position::byte_position_at_end(&sql[..at]),
                0,
            )
            .unwrap();
        assert!(scope.aliases.iter().any(|a| a.name == "o"));
        assert_eq!(scope.aliases.iter().any(|a| a.name == "a"), index == 1);
        assert!(!scope.aliases.iter().any(|a| a.name == "b"));
    }
}
#[test]
fn hive_lateral_view_source_budgets_and_missing_aliases_remain_conservative() {
    let mut parser = SqlParser::new();
    for sql in [
        "SELECT * FROM orders LATERAL VIEW explode(tags)",
        "SELECT * FROM orders LATERAL VIEW explode(tags) e AS",
    ] {
        let parsed = parser.parse(sql);
        assert!(
            parsed.tree.as_ref().unwrap().root_node().has_error(),
            "{sql}"
        );
    }
    let sql = format!(
        "SELECT * FROM orders {}",
        (0..65)
            .map(|n| format!("LATERAL VIEW explode(tags) e{n} AS item{n}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let tree = parser.parse(&sql).tree.unwrap();
    assert!(SqlParser::lateral_views(&tree, &sql).is_empty());
}
