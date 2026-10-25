use super::*;
use crate::SqlParser;
use tower_lsp::lsp_types::{HoverParams, NumberOrString};

fn scope(marked: &str) -> Vec<crate::parser::RelationScope> {
    let at = marked.find("<cursor>").unwrap();
    let sql = marked.replace("<cursor>", "");
    let mut parser = SqlParser::new();
    let tree = parser.parse_syntax_tree(&sql).unwrap();
    parser.relation_scopes_at_position(
        &tree,
        &sql,
        crate::position::byte_position_at_end(&sql[..at]),
    )
}
fn aliases(marked: &str, expected: &[&str]) {
    let scopes = scope(marked);
    let names: Vec<_> = scopes[0].aliases.iter().map(|a| a.name.as_str()).collect();
    if names != expected {
        let sql = marked.replace("<cursor>", "");
        let tree = SqlParser::new().parse_syntax_tree(&sql).unwrap();
        panic!(
            "{marked}: {names:?} != {expected:?}: {}",
            tree.root_node().to_sexp()
        );
    }
}
#[test]
fn join_operands_do_not_import_later_sources() {
    aliases(
        "SELECT o.id FROM orders o JOIN users u ON id<cursor>=u.id JOIN audit_events a ON true;",
        &["o", "u"],
    );
}
#[test]
fn join_operands_comma_has_lower_precedence() {
    aliases(
        "SELECT o.id FROM audit_events a, orders o JOIN users u ON id<cursor>=u.id;",
        &["o", "u"],
    );
}
#[test]
fn join_operands_keep_accumulated_left_join_chain() {
    aliases("SELECT o.id FROM orders o JOIN users u ON o.id=u.id JOIN audit_events a ON id<cursor>=1 JOIN orders later ON true;", &["o", "u", "a"]);
}
#[test]
fn join_operands_scalar_correlation_keeps_parent_on_boundary() {
    let scopes = scope("SELECT o.id FROM orders o JOIN users u ON EXISTS(SELECT 1 WHERE id<cursor>=1) JOIN audit_events a ON true;");
    assert!(scopes[0].relations.is_empty());
    let names: Vec<_> = scopes[1].aliases.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["o", "u"]);
}
#[test]
fn join_operands_do_not_narrow_select_or_where() {
    for sql in ["SELECT id<cursor> FROM orders o JOIN users u ON true JOIN audit_events a ON true;", "SELECT o.id FROM orders o JOIN users u ON true JOIN audit_events a ON true WHERE id<cursor>=1;"] {
        let scopes = scope(sql);
        assert_eq!(scopes[0].aliases.len(), 3, "{sql}");
    }
}
#[test]
fn join_operands_diagnostic_uses_each_field_boundary() {
    let schema = corpus_schema();
    for sql in [
        "SELECT o.id FROM orders o JOIN users u ON total=u.id JOIN orders later ON later.id=u.id;",
        "SELECT o.id FROM orders hidden, users u JOIN orders o ON total=u.id;",
        "SELECT o.id FROM orders o JOIN users u ON EXISTS(SELECT 1 WHERE total=u.id) JOIN orders later ON later.id=u.id;",
    ] {
        let ds = sql_inspection_diagnostics(sql, Some(&schema), "postgres");
        assert!(!ds.iter().any(|d| d.code==Some(NumberOrString::String("OXIDE004".into()))), "{sql}: {ds:?}");
    }
}
#[tokio::test]
async fn join_operands_completion_excludes_future_and_comma_sources() {
    for dialect in ["postgres", "mysql", "hive", "clickhouse", "sqlserver"] {
        for sql in [
            "SELECT o.id FROM orders o JOIN users u ON audit_<cursor>='x' JOIN audit_events a ON true;",
            "SELECT o.id FROM audit_events a, orders o JOIN users u ON audit_<cursor>='x';",
            "SELECT o.id FROM orders o JOIN users u ON a.<cursor>=1 JOIN audit_events a ON true;",
        ] {
            let items = candidates(dialect, sql).await;
            assert!(!items.iter().any(|i| i.kind==Some(CompletionItemKind::FIELD) && i.label.contains("audit_secret")), "{dialect} {sql}: {items:?}");
        }
    }
}
#[tokio::test]
async fn join_operands_hover_resolves_only_current_inputs() {
    let h = configured("postgres", "SELECT o.id FROM orders o JOIN audit_events a ON i<cursor>d=1 JOIN users later ON later.id=o.id;");
    let params: HoverParams = serde_json::from_value(
        serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
    )
    .unwrap();
    let hover = h.service.inner().hover(params).await.unwrap();
    assert!(
        serde_json::to_string(&hover).unwrap().contains("bigint"),
        "{hover:?}"
    );
}
#[test]
fn join_operands_quickfix_excludes_future_alias_of_same_table() {
    let sql =
        "SELECT o.id FROM orders o JOIN orders p ON id=p.id JOIN orders later ON later.id=p.id;";
    let at = sql.find("ON id").unwrap() + 5;
    let pos = lsp_position_at_end(&sql[..at]);
    let uri = Url::parse("oxide://join/choices.sql").unwrap();
    let actions = qualify_identifier_actions(
        sql,
        &uri,
        Range::new(pos, pos),
        Some(&corpus_schema()),
        "postgres",
    );
    let titles: Vec<_> = actions.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, ["Qualify column as o.id", "Qualify column as p.id"]);
}

#[tokio::test]
async fn join_operands_alias_recommendations_do_not_offer_later_aliases() {
    for dialect in ["postgres", "mysql", "hive", "clickhouse", "sqlserver"] {
        let items = candidates(dialect,"SELECT o.id FROM orders o JOIN users u ON la<cursor>=1 JOIN audit_events later ON true;").await;
        assert!(
            !items
                .iter()
                .any(|i| i.kind == Some(CompletionItemKind::VARIABLE) && i.label == "later"),
            "{dialect}: {items:?}"
        );
    }
}

#[tokio::test]
async fn join_operands_completion_does_not_duplicate_same_table_future_alias() {
    for dialect in ["postgres", "mysql", "hive", "clickhouse", "sqlserver"] {
        let items = candidates(dialect,"SELECT o.id FROM orders o JOIN users u ON tot<cursor>=1 JOIN orders later ON later.id=u.id;").await;
        let fields: Vec<_> = items
            .iter()
            .filter(|i| i.kind == Some(CompletionItemKind::FIELD))
            .collect();
        assert_eq!(fields.len(), 1, "{dialect}: {fields:?}");
        assert!(
            matches!(fields[0].label.as_str(), "total" | "o.total"),
            "{dialect}: {fields:?}"
        );
        assert!(fields[0]
            .detail
            .as_deref()
            .is_some_and(|s| s.contains("decimal")));
    }
}

#[test]
fn join_operands_quoted_rows_keep_original_spelling_and_byte_offsets() {
    let scopes = scope("/*😀*/\r\nSELECT \"O 表\".id FROM orders AS \"O 表\" JOIN users AS \"U\" ON id<cursor>=\"U\".id JOIN audit_events later ON true;");
    let names: Vec<_> = scopes[0].aliases.iter().map(|a| a.sql.as_str()).collect();
    assert_eq!(names, ["\"O 表\"", "\"U\""]);
}

#[tokio::test]
async fn join_operands_local_cte_and_derived_rows_remain_typed() {
    for sql in [
        "WITH q AS(SELECT total AS amount FROM orders) SELECT u.id FROM users u JOIN q ON am<cursor>ount>0 JOIN audit_events later ON true;",
        "SELECT u.id FROM users u JOIN (SELECT total AS amount FROM orders) q ON am<cursor>ount>0 JOIN audit_events later ON true;",
    ] {
        let h=configured("postgres",sql);
        let params=serde_json::from_value(serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position})).unwrap();
        let hover=h.service.inner().hover(params).await.unwrap();
        assert!(serde_json::to_string(&hover).unwrap().contains("decimal"),"{sql}: {hover:?}");
        let items=complete_harness(&h).await;
        assert!(items.iter().any(|i|matches!(i.label.as_str(),"amount"|"q.amount") && i.detail.as_deref().is_some_and(|s|s.contains("decimal"))),"{sql}: {items:?}");
    }
}

#[test]
fn join_operands_incomplete_on_does_not_import_future_relation() {
    aliases(
        "SELECT o.id FROM orders o JOIN users u ON <cursor> JOIN audit_events later ON true;",
        &["o", "u"],
    );
}

#[test]
fn join_operands_nested_scalar_edges_keep_each_ancestor_boundary() {
    let scopes=scope("SELECT o.id FROM orders o JOIN users u ON EXISTS(SELECT 1 FROM audit_events a WHERE EXISTS(SELECT id<cursor>)) JOIN orders later ON true;");
    assert_eq!(scopes.len(), 3);
    assert_eq!(scopes[1].relations, ["audit_events"]);
    let names: Vec<_> = scopes[2].aliases.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["o", "u"]);
}

#[tokio::test]
async fn join_operands_mixed_unaliased_self_join_keeps_both_choices() {
    let marked =
        "SELECT p.id FROM orders JOIN orders p ON i<cursor>d=p.id JOIN audit_events later ON true;";
    let h = configured("postgres", marked);
    let hover = h
        .service
        .inner()
        .hover(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        hover.is_none(),
        "a table and an alias of it are distinct rows"
    );
    let items = complete_harness(&h).await;
    let labels: Vec<_> = items
        .iter()
        .filter(|i| i.kind == Some(CompletionItemKind::FIELD))
        .map(|i| i.label.as_str())
        .collect();
    assert!(
        labels.contains(&"orders.id") && labels.contains(&"p.id"),
        "{labels:?}"
    );
    assert!(!labels.contains(&"id"));
    let sql = marked.replace("<cursor>", "");
    let pos = lsp_position_at_end(&sql[..sql.find("ON id").unwrap() + 5]);
    let actions = qualify_identifier_actions(
        &sql,
        &h.uri,
        Range::new(pos, pos),
        Some(&corpus_schema()),
        "postgres",
    );
    let titles: Vec<_> = actions.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Qualify column as orders.id", "Qualify column as p.id"]
    );
}

#[tokio::test]
async fn join_operands_sqlite_keeps_comma_and_inner_future_sources() {
    for dialect in ["sqlite", "sqlite3", "cloudflare-d1", "turso"] {
        for sql in [
            "SELECT o.id FROM audit_events a, orders o JOIN users u ON audit_<cursor>='x';",
            "SELECT o.id FROM orders o JOIN users u ON audit_<cursor>='x' JOIN audit_events later ON true;",
            "SELECT o.id FROM audit_events a, orders o LEFT JOIN users u ON audit_<cursor>='x';",
        ] {
            let items=candidates(dialect,sql).await;
            assert!(items.iter().any(|i|i.label=="audit_secret"),"{dialect} {sql}: {items:?}");
        }
        let items=candidates(dialect,"SELECT o.id FROM orders o LEFT JOIN users u ON audit_<cursor>='x' JOIN audit_events later ON true;").await;
        assert!(
            !items
                .iter()
                .any(|i| i.kind == Some(CompletionItemKind::FIELD)),
            "{dialect}: {items:?}"
        );
    }
    let items = candidates(
        "duckdb",
        "SELECT o.id FROM audit_events a, orders o JOIN users u ON audit_<cursor>='x';",
    )
    .await;
    assert!(
        !items
            .iter()
            .any(|i| i.kind == Some(CompletionItemKind::FIELD)),
        "DuckDB keeps ANSI precedence: {items:?}"
    );
}
#[tokio::test]
async fn join_operands_sqlite_inner_future_overlap_remains_ambiguous() {
    let sql="SELECT o.id FROM orders o JOIN users u ON tot<cursor>al=1 JOIN orders later ON later.id=u.id;";
    let items = candidates("sqlite", sql).await;
    let labels: Vec<_> = items
        .iter()
        .filter(|i| i.kind == Some(CompletionItemKind::FIELD))
        .map(|i| i.label.as_str())
        .collect();
    assert_eq!(labels, ["o.total", "later.total"]);
    let h = configured("sqlite", sql);
    let hover = h
        .service
        .inner()
        .hover(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(hover.is_none(), "{hover:?}");
    let sql="SELECT o.id FROM orders o LEFT JOIN users u ON tot<cursor>al=1 JOIN orders later ON later.id=u.id;";
    let h = configured("sqlite", sql);
    let items = complete_harness(&h).await;
    assert!(items.iter().any(|i| i.label == "total"));
    let hover = h
        .service
        .inner()
        .hover(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(serde_json::to_string(&hover).unwrap().contains("decimal"));
}

#[tokio::test]
async fn join_operands_qualified_scalar_correlation_preserves_shadow_and_on_boundary() {
    for dialect in [
        "postgres",
        "mysql",
        "sqlite",
        "hive",
        "clickhouse",
        "sqlserver",
    ] {
        for (sql,name,kind) in [
            ("SELECT o.id FROM orders o LEFT JOIN users u ON EXISTS(SELECT o.to<cursor>tal) JOIN orders later ON true;",Some("total"),"decimal"),
            ("SELECT o.id FROM orders o LEFT JOIN users u ON EXISTS(SELECT later.to<cursor>tal) JOIN orders later ON true;",None,""),
            ("SELECT o.id FROM orders o LEFT JOIN users u ON EXISTS(SELECT o.na<cursor>me FROM users o) JOIN orders later ON true;",Some("name"),"text"),
            ("SELECT o.id FROM orders o LEFT JOIN users u ON EXISTS(SELECT o.to<cursor>tal FROM users o) JOIN orders later ON true;",Some("name"),"text"),
        ] {
            let items=candidates(dialect,sql).await;let fields:Vec<_>=items.iter().filter(|i|i.kind==Some(CompletionItemKind::FIELD)).collect();
            if let Some(name)=name {
                let catalog=corpus_schema();
                let table=if name=="total" {"orders"} else {"users"};
                let mut expected:Vec<_>=catalog.tables.iter().find(|t|t.name==table).unwrap().columns.iter().map(|c|c.name.as_str()).collect();
                let mut actual:Vec<_>=fields.iter().map(|i|i.label.as_str()).collect();
                expected.sort_unstable();actual.sort_unstable();assert_eq!(actual,expected,"{dialect} {sql}");
                let item=fields.iter().find(|i|i.label==name).unwrap();
                assert!(item.detail.as_deref().is_some_and(|d|d.contains(kind)),"{fields:?}");
            } else {assert!(fields.is_empty(),"{dialect} {sql}: {fields:?}");}
        }
    }
}
