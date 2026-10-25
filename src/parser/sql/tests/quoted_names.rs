use super::*;
#[test]
fn quoted_path_segments_keep_embedded_dots_and_escapes_atomic() {
    for (sql, parts) in [
        (
            r#""app.name"."odd.table"."display.name""#,
            vec!["app.name", "odd.table", "display.name"],
        ),
        (
            "`app.name`.`odd``table`.`display.name`",
            vec!["app.name", "odd`table", "display.name"],
        ),
        (
            "[app.name].[odd]]table].[display.name]",
            vec!["app.name", "odd]table", "display.name"],
        ),
    ] {
        assert_eq!(SqlParser::identifier_parts(sql), parts, "{sql}");
        assert_eq!(
            SqlParser::identifier_parts(&SqlParser::normalize_identifier(sql)),
            parts,
            "round trip {sql}"
        );
    }
}
#[test]
fn qualified_path_is_distinct_from_single_quoted_component() {
    assert_eq!(SqlParser::identifier_parts(r#""app.users""#), ["app.users"]);
    assert_eq!(SqlParser::identifier_parts("app.users"), ["app", "users"]);
    assert_ne!(
        SqlParser::normalize_identifier(r#""app.users""#),
        SqlParser::normalize_identifier("app.users")
    );
    assert!(SqlParser::table_name_matches(
        r#""app.name"."odd.table""#,
        "app.name",
        "odd.table"
    ));
    assert!(!SqlParser::table_name_matches(
        "app.name.odd.table",
        "app.name",
        "odd.table"
    ));
}
#[test]
fn escaped_bracket_reader_keeps_whole_identifier() {
    assert_eq!(
        SqlParser::read_identifier_part("[odd]]name] tail", 0),
        Some(("odd]name".into(), 11))
    );
}

#[test]
fn escaped_postgres_names_keep_tree_coordinates_and_source() {
    let sql = "-- 😀\r\nSELECT u.\"quote\"\"name\" FROM users u";
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let offset = sql.find("name").unwrap() + 2;
    let pos = SqlParser::lsp_position_to_byte_position(
        sql,
        crate::position::lsp_position_at_end(&sql[..offset]),
    );
    let node = parser
        .get_node_at_position(result.tree.as_ref().unwrap(), pos)
        .unwrap();
    assert_eq!(parser.node_text(node, sql), r#""quote""name""#);
    assert_eq!(
        SqlParser::identifier_last_part(&parser.node_text(node, sql)),
        "quote\"name"
    );
}
#[test]
fn escaped_name_adapter_preserves_strings_comments_and_array_indexing() {
    use crate::placeholder::SqlPlaceholderDialect;
    let sql = r#"SELECT 'quote""name', a[1], "ordinary name" /* "quoted""comment" */ FROM users"#;
    assert_eq!(
        super::super::normalization::normalize_for_parser(sql, SqlPlaceholderDialect::Postgres),
        sql
    );
    let mysql = r#"SELECT "quote""name" FROM users"#;
    assert_eq!(
        super::super::normalization::normalize_for_parser(mysql, SqlPlaceholderDialect::Mysql),
        mysql
    );
}
