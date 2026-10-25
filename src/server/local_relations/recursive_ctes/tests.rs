use super::*;
use crate::{
    parser::SqlParser,
    schema::{SchemaId, Table},
};

fn shape(dialect: &str, name: &str, quoted: bool, names: &[&str], body: &str) -> Option<Shape> {
    let names: Vec<_> = names.iter().map(|name| name.to_string()).collect();
    let upper = SqlParser::mask_sql_noise(body).to_ascii_uppercase();
    let schema = Schema {
        id: SchemaId::new(),
        database: "public".into(),
        tables: vec![Table {
            name: "users".into(),
            columns: vec![Column {
                name: "id".into(),
                data_type: "integer".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        server: None,
        namespace: None,
        catalog: None,
        server_version: None,
        functions: vec![],
        source_uri: None,
    };
    infer(
        &Definition {
            name,
            quoted,
            names: &names,
            body,
            upper: &upper,
            line: 1,
        },
        &schema,
        "oxide://query/recursive.sql",
        Some(dialect),
        Default::default(),
    )
}

#[test]
fn recursive_anchor_contract_preserves_engine_specific_types_and_nullability() {
    for (dialect, anchor, member, ty) in [
        ("postgres", "u.id", "w.n+1", "integer"),
        ("mysql", "u.id", "CAST(w.n AS SIGNED)", "integer"),
        ("sqlserver", "CAST(u.id AS int)", "w.n+1", "int"),
    ] {
        let shape = shape(
            dialect,
            "walk",
            false,
            &["n"],
            &format!("SELECT {anchor} FROM users u UNION ALL SELECT {member} FROM walk w"),
        )
        .unwrap();
        assert_eq!(shape.columns.len(), 1);
        assert_eq!(shape.columns[0].data_type, ty, "{dialect}");
        assert!(shape.columns[0].nullable);
    }
}

#[test]
fn recursive_invalid_width_or_known_type_conflict_does_not_promise_a_type() {
    for (dialect,names,body) in [
        ("postgres",vec!["n"],"SELECT u.id FROM users u UNION ALL SELECT w.n,w.n FROM walk w"),
        ("postgres",vec!["n"],"SELECT u.id FROM users u UNION ALL SELECT CAST(w.n AS bigint) FROM walk w"),
        ("mysql",vec!["n","extra"],"SELECT u.id FROM users u UNION ALL SELECT w.n FROM walk w"),
        ("sqlserver",vec!["n"],"SELECT CAST(u.id AS int) FROM users u UNION ALL SELECT CAST(w.n AS bigint) FROM walk w"),
        ("sqlite",vec!["n"],"SELECT u.id FROM users u UNION ALL SELECT 'different type' FROM walk w"),
    ] {
        let result=shape(dialect,"walk",false,&names,body).unwrap();
        assert!(result.columns.iter().all(|column|column.data_type=="unknown"),"{dialect}: {body}: {:?}",result.columns);
    }
}

#[test]
fn recursive_reference_detection_respects_qualification_quotes_and_nested_with() {
    for (name, quoted, body) in [
        (
            "walk",
            false,
            "SELECT 1 UNION ALL SELECT n FROM public.walk",
        ),
        ("walk", false, "SELECT 1 UNION ALL SELECT 'FROM walk'"),
        ("walk", false, "SELECT 1 UNION ALL SELECT 2 /* FROM walk */"),
        ("Walk", true, "SELECT 1 UNION ALL SELECT n FROM Walk"),
        (
            "walk",
            false,
            "SELECT 1 UNION ALL (WITH walk AS (SELECT 2 AS n) SELECT n FROM walk)",
        ),
    ] {
        assert!(
            shape("postgres", name, quoted, &["n"], body).is_none(),
            "{body}"
        );
    }
    assert!(shape(
        "postgres",
        "Walk",
        true,
        &["n"],
        "SELECT 1 UNION ALL SELECT w.n FROM \"Walk\" w"
    )
    .is_some());
}

#[test]
fn recursive_parentheses_and_trivia_preserve_member_scope() {
    for body in [
        "(SELECT 1) UNION ALL (SELECT w.n+1 FROM walk w)",
        " /* anchor */ ((SELECT 1 UNION ALL SELECT w.n+1 FROM walk w)) ",
        "SELECT 1 /* UNION ALL FROM walk */ UNION /* mode */ ALL SELECT w.n+1 FROM /* binding */ walk w",
    ] {
        let result=shape("postgres","walk",false,&["n"],body).unwrap();
        assert_eq!(result.columns[0].data_type,"integer","{body}");
        assert!(result.member_start<body.find("w.n").unwrap());
    }
}

fn balanced(count: usize) -> String {
    if count == 1 {
        return "SELECT 1".into();
    }
    format!(
        "({} UNION ALL {})",
        balanced(count / 2),
        balanced(count - count / 2)
    )
}

#[test]
fn recursive_anchor_and_member_share_the_exact_branch_budget() {
    for (anchors, expected) in [(63, "integer"), (64, "unknown")] {
        let body = format!("{} UNION ALL SELECT w.n FROM walk w", balanced(anchors));
        let result = shape("postgres", "walk", false, &["n"], &body).unwrap();
        assert_eq!(result.columns[0].data_type, expected, "{anchors}");
    }
}

#[test]
fn unanchored_self_reference_exposes_only_declared_unknown_columns() {
    let result = shape(
        "postgres",
        "users",
        false,
        &["n"],
        "SELECT u.* FROM users u",
    )
    .unwrap();
    assert_eq!(result.columns.len(), 1);
    assert_eq!(result.columns[0].name, "n");
    assert_eq!(result.columns[0].data_type, "unknown");
}

#[test]
fn recursive_parenthesis_and_anchor_depths_share_the_query_limit() {
    let anchor = format!("{}SELECT 1{}", "(".repeat(20), ")".repeat(20));
    let body = format!(
        "{}{anchor} UNION ALL SELECT w.n FROM walk w{}",
        "(".repeat(20),
        ")".repeat(20)
    );
    let result = shape("postgres", "walk", false, &["n"], &body).unwrap();
    assert_eq!(result.columns[0].data_type, "unknown");
}
