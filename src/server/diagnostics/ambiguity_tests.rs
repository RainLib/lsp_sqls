use super::*;
use std::time::Instant;
#[test]
fn missing_on_recovery_does_not_merge_independent_statement_bindings() {
    let schema:Schema=serde_json::from_value(serde_json::json!({"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","tables":[{"name":"orders","columns":[{"name":"id","data_type":"bigint","nullable":false}]},{"name":"customers","columns":[{"name":"id","data_type":"bigint","nullable":false}]}],"functions":[]})).unwrap();
    let sql = "SELECT id FROM orders JOIN customers WHERE orders.id>0; SELECT id FROM customers;";
    let tree = prepare(sql);
    let ctx = InspectionContext::with_syntax(sql, "mysql", Some(&tree));
    let (queries, _) = graphs(&ctx, &tree);
    assert_eq!(queries.len(), 2);
    let semicolon = sql.find(';').unwrap();
    assert!(queries[0].range.end <= semicolon + 1);
    assert!(queries[1].range.start > semicolon);
    let diagnostics = super::super::sql_inspection_diagnostics(sql, Some(&schema), "mysql");
    let ambiguous: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.code == Some(NumberOrString::String("OXIDE004".into())))
        .collect();
    assert_eq!(ambiguous.len(), 1, "{diagnostics:?}");
    assert_eq!(ambiguous[0].range.start.character, 7);
}

fn catalog() -> Schema {
    serde_json::from_value(serde_json::json!({"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","tables":[{"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false}]}],"functions":[]})).unwrap()
}
#[test]
fn cte_star_and_catalog_rows_share_ambiguity_binding() {
    let schema = catalog();
    for dialect in ["postgres", "mysql", "sqlserver", "sqlite", "hive"] {
        for sql in [
            "WITH q AS (SELECT u.* FROM users u) SELECT id FROM q a JOIN users b ON 1=1;",
            "WITH q AS (SELECT id FROM users) SELECT id FROM q a JOIN users b ON 1=1;",
            "WITH seed AS (SELECT u.* FROM users u), q AS (SELECT s.* FROM seed s) SELECT id FROM q a JOIN users b ON 1=1;",
            "SELECT id FROM (SELECT u.* FROM users u) a JOIN users b ON 1=1;",
        ] {
            let diagnostics=super::super::sql_inspection_diagnostics(sql,Some(&schema),dialect);
            assert_eq!(diagnostics.iter().filter(|diagnostic|diagnostic.code==Some(NumberOrString::String("OXIDE004".into()))).count(),1,"{dialect}: {sql}: {diagnostics:?}");
        }
    }
}
#[test]
fn aliased_local_rows_do_not_manufacture_ambiguity_or_cross_query_blocks() {
    let schema = catalog();
    for dialect in ["postgres", "mysql", "sqlserver", "sqlite", "hive"] {
        for sql in [
            "WITH q AS (SELECT u.* FROM users u) SELECT a.id FROM q a JOIN users b ON 1=1;",
            "WITH q AS (SELECT 1 AS other) SELECT id FROM q a JOIN users b ON 1=1;",
            "WITH q AS (SELECT u.* FROM users u) SELECT id FROM users b WHERE EXISTS(SELECT id FROM q a);",
            "WITH q AS (SELECT * FROM missing) SELECT id FROM q a JOIN users b ON 1=1;",
        ] {
            let diagnostics=super::super::sql_inspection_diagnostics(sql,Some(&schema),dialect);
            assert!(!diagnostics.iter().any(|diagnostic|diagnostic.code==Some(NumberOrString::String("OXIDE004".into()))),"{dialect}: {sql}: {diagnostics:?}");
        }
    }
}
#[test]
fn aliased_cte_join_keys_remain_merged_in_supporting_profiles() {
    let schema = catalog();
    for dialect in ["postgres", "mysql", "sqlite"] {
        for join in ["JOIN users b USING(id)", "NATURAL JOIN users b"] {
            let sql = format!("WITH q AS (SELECT u.* FROM users u) SELECT id FROM q a {join};");
            let diagnostics =
                super::super::sql_inspection_diagnostics(&sql, Some(&schema), dialect);
            assert!(
                !diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code
                        == Some(NumberOrString::String("OXIDE004".into()))),
                "{dialect}: {sql}: {diagnostics:?}"
            );
        }
    }
}
fn prepare(sql: &str) -> Tree {
    SqlParser::new().parse_syntax_tree(sql).unwrap()
}
#[test]
fn diagnostic_profile_graph_walk_is_one_pass_for_many_independent_mutations() {
    let sql = format!("/*😀*/\r\n{}", "UPDATE users SET id=1; ".repeat(4097));
    let tree = prepare(&sql);
    let ctx = InspectionContext::with_syntax(&sql, "mysql", Some(&tree));
    let mut nodes = 0;
    walk(&tree, |_| nodes += 1);
    WALK_VISITS.with(|n| n.set(0));
    let (queries, ctes) = graphs(&ctx, &tree);
    assert_eq!(queries.len(), 4097);
    assert!(ctes.is_empty());
    assert!(queries
        .iter()
        .all(|q| q.parent.is_none() && q.fields.is_empty()));
    assert!(
        WALK_VISITS.with(|n| n.get()) <= nodes,
        "the query model revisited the full tree: {} > {nodes}",
        WALK_VISITS.with(|n| n.get())
    );
}
#[test]
fn diagnostic_profile_cte_scalar_and_set_fields_keep_query_ownership() {
    let sql="WITH q AS(SELECT id FROM users) SELECT total,(SELECT name FROM customers WHERE id>0) AS value FROM orders UNION ALL SELECT id FROM third;";
    let tree = prepare(sql);
    let ctx = InspectionContext::with_syntax(sql, "postgres", Some(&tree));
    let (queries, ctes) = graphs(&ctx, &tree);
    assert_eq!(queries.len(), 4);
    assert_eq!(ctes.len(), 1);
    let names: Vec<Vec<_>> = queries
        .iter()
        .map(|q| q.fields.iter().map(|f| f.name.as_str()).collect())
        .collect();
    assert_eq!(
        names,
        [vec!["id"], vec!["total"], vec!["name", "id"], vec!["id"]]
    );
    assert_eq!(
        queries.iter().map(|q| q.parent).collect::<Vec<_>>(),
        [None, None, Some(1), None]
    );
    assert!(queries[0].isolated);
}
#[test]
fn diagnostic_profile_nested_mutations_keep_parent_and_statement_boundaries() {
    let sql="WITH q AS(SELECT id FROM users) UPDATE orders SET total=(SELECT id FROM q); SELECT id FROM orders WHERE EXISTS(SELECT id FROM users);";
    let tree = prepare(sql);
    let ctx = InspectionContext::with_syntax(sql, "postgres", Some(&tree));
    let (queries, _) = graphs(&ctx, &tree);
    assert_eq!(queries.len(), 5);
    assert_eq!(
        queries.iter().map(|q| q.parent).collect::<Vec<_>>(),
        [None, None, Some(1), None, Some(3)]
    );
    let names: Vec<Vec<_>> = queries
        .iter()
        .map(|q| q.fields.iter().map(|f| f.name.as_str()).collect())
        .collect();
    assert_eq!(
        names,
        [vec!["id"], vec![], vec!["id"], vec!["id"], vec!["id"]]
    );
}
#[test]
fn diagnostic_profile_phases_preserve_all_large_warning_payloads() {
    let sql = format!("/*😀*/\r\n{}", "UPDATE users SET id=1; ".repeat(4097));
    let schema = catalog();
    let mut parser = SqlParser::new();
    for round in 0..5 {
        let t = Instant::now();
        let parsed = parser.parse(&sql);
        let parse_ms = t.elapsed().as_secs_f64() * 1000.;
        assert!(parsed.diagnostics.is_empty());
        let tree = parsed.tree.as_ref().unwrap();
        let t = Instant::now();
        let ctx = InspectionContext::with_syntax(&sql, "mysql", Some(tree));
        let context_ms = t.elapsed().as_secs_f64() * 1000.;
        let t = Instant::now();
        let mutations: usize = ctx.statements().map(|s| ctx.mutations(&s).count()).sum();
        let structure_ms = t.elapsed().as_secs_f64() * 1000.;
        assert_eq!(mutations, 4097);
        let t = Instant::now();
        let ambiguity = diagnostics(&ctx, Some(&schema));
        let ambiguity_ms = t.elapsed().as_secs_f64() * 1000.;
        assert!(ambiguity.is_empty());
        let t = Instant::now();
        let items = super::super::sql_inspection_diagnostics_with_syntax(
            &sql,
            Some(&schema),
            "mysql",
            Some(tree),
        );
        let inspections_ms = t.elapsed().as_secs_f64() * 1000.;
        assert_eq!(items.len(), 4097);
        assert!(items
            .iter()
            .all(|d| d.code == Some(NumberOrString::String("OXIDE001".into()))));
        assert_eq!(items[0].range.start.line, 0);
        assert_eq!(items.last().unwrap().range.end.line, 1);
        let t = Instant::now();
        let encoded = serde_json::to_vec(&items).unwrap();
        let json_ms = t.elapsed().as_secs_f64() * 1000.;
        assert!(encoded.len() > 500000);
        println!(
            "{}",
            serde_json::json!({"round":round+1,"profile":"debug-test-instrumented-not-release-latency","parseMs":parse_ms,"contextMs":context_ms,"structureMs":structure_ms,"ambiguityMs":ambiguity_ms,"allInspectionsMs":inspections_ms,"jsonMs":json_ms,"items":items.len(),"jsonBytes":encoded.len()})
        );
    }
}
