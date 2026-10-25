use super::*;
use crate::parser::SqlParser;

fn schema() -> Schema {
    Schema {
        id: Default::default(),
        server: None,
        namespace: None,
        catalog: None,
        database: "app".into(),
        server_version: None,
        tables: vec![
            row("a", &[("id", "integer"), ("left_value", "text")]),
            row("b", &[("id", "bigint"), ("right_value", "decimal")]),
            row("g", &[("shadow_secret", "text")]),
        ],
        functions: vec![],
        source_uri: None,
    }
}
fn row(name: &str, columns: &[(&str, &str)]) -> Table {
    Table {
        name: name.into(),
        columns: columns
            .iter()
            .map(|(n, t)| Column {
                name: (*n).into(),
                data_type: (*t).into(),
                nullable: false,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}
fn bind<'a>(schema: &'a Schema, sql: &str) -> JoinedSources<'a> {
    let mut parser = SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::Postgres,
    );
    let tree = parser.parse_syntax_tree(sql).unwrap();
    assert!(
        !tree.root_node().has_error(),
        "{}",
        tree.root_node().to_sexp()
    );
    let cursor = sql.find(") SELECT").map_or(8, |at| at + 10);
    let scope = parser
        .relation_scopes_at_position(
            &tree,
            sql,
            crate::position::byte_position_at_end(&sql[..cursor]),
        )
        .remove(0);
    resolve_joined_sources(
        scope.joined.as_deref().expect("real join layout"),
        schema,
        "postgres",
    )
}
#[test]
fn joined_tables_using_order_type_and_null_extension_are_shared() {
    let schema = schema();
    for (join, left_nullable, right_nullable) in [
        ("JOIN", false, false),
        ("LEFT JOIN", false, true),
        ("RIGHT JOIN", true, false),
        ("FULL JOIN", true, true),
    ] {
        let result = bind(&schema, &format!("SELECT * FROM a {join} b USING(id)"));
        let output = result.output.unwrap();
        assert_eq!(
            output
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["id", "left_value", "right_value"]
        );
        assert_eq!(output.columns[0].data_type, "bigint");
        assert!(!output.columns[0].nullable);
        assert_eq!(output.columns[1].nullable, left_nullable);
        assert_eq!(output.columns[2].nullable, right_nullable);
        assert_eq!(
            result.rows[0].table.as_ref().unwrap().columns[0].nullable,
            left_nullable
        );
        assert_eq!(
            result.rows[1].table.as_ref().unwrap().columns[0].nullable,
            right_nullable
        );
    }
    assert!(
        !schema.tables[0].columns[0].nullable,
        "null extension never mutates catalog"
    );
}
#[test]
fn joined_tables_group_column_list_and_duplicate_names_are_preserved() {
    let schema = schema();
    let result = bind(&schema, "SELECT * FROM (a JOIN b USING(id)) g(key,caption)");
    let table = result.rows[0].table.as_ref().unwrap();
    assert_eq!(table.name, "g");
    assert_eq!(
        table
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["key", "caption", "right_value"]
    );
    let result = bind(&schema, "SELECT * FROM (a JOIN b ON true) g");
    assert_eq!(
        result
            .output
            .unwrap()
            .columns
            .iter()
            .filter(|c| c.name == "id")
            .count(),
        2
    );
}
#[test]
fn joined_tables_unknown_local_input_never_uses_physical_shadow_or_partial_star() {
    let schema = schema();
    for sql in [
        "SELECT * FROM (a JOIN missing ON true) g",
        "WITH g AS(SELECT * FROM missing) SELECT * FROM g JOIN a USING(id)",
        "SELECT * FROM (SELECT * FROM missing) g JOIN a USING(id)",
    ] {
        let result = bind(&schema, sql);
        assert!(result.output.is_none(), "{sql}");
        assert!(
            !result
                .rows
                .iter()
                .filter_map(|r| r.table.as_deref())
                .any(|t| t.columns.iter().any(|c| c.name == "shadow_secret")),
            "{sql}"
        );
    }
}
#[test]
fn joined_tables_alias_column_list_with_wrong_arity_remains_unknown() {
    let schema = schema();
    let result = bind(&schema, "SELECT * FROM (a JOIN b USING(id)) g(w,x,y,z)");
    assert!(result.output.is_none());
    assert!(result.rows[0].table.is_none());
}
#[test]
fn joined_tables_hidden_columns_are_not_implicit_natural_keys() {
    let mut schema = schema();
    schema.tables[0].columns[0].hidden = true;
    let result = bind(&schema, "SELECT * FROM a NATURAL JOIN b");
    let output = result.output.unwrap();
    assert_eq!(output.columns.iter().filter(|c| c.name == "id").count(), 2);
}

#[test]
fn joined_tables_invalid_using_keys_never_fabricate_output_shape() {
    let schema = schema();
    for sql in [
        "SELECT * FROM a JOIN b USING(missing)",
        "SELECT * FROM a JOIN b USING(id,id)",
        "SELECT * FROM (a JOIN b ON true) x JOIN a USING(id)",
    ] {
        assert!(bind(&schema, sql).output.is_none(), "{sql}");
    }
}
