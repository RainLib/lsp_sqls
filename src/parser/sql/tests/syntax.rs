use super::*;

#[test]
fn incremental_parse_reuses_an_edited_tree_without_shifting_placeholders() {
    let original = "SELECT * FROM users WHERE id = $1";
    let updated = "SELECT email FROM users WHERE id = $1 AND active = true";
    let mut parser = SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::Postgres,
    );
    let original_result = parser.parse(original);
    let updated_result = parser.parse_incremental(
        updated,
        original,
        original_result.tree.as_ref().expect("initial tree"),
    );
    let tree = updated_result.tree.as_ref().expect("updated tree");
    assert_eq!(
        parser.extract_referenced_tables(tree, updated),
        vec!["users"]
    );
    assert!(updated_result.diagnostics.is_empty());

    let edit = minimal_input_edit("SELECT 😀", "SELECT 😀, id");
    assert_eq!(edit.start_byte, "SELECT 😀".len());
    assert_eq!(edit.old_end_position.column, "SELECT 😀".len());
}

#[test]
fn normalizes_quoted_identifier_paths() {
    assert_eq!(
        SqlParser::normalize_identifier(r#""public"."users""#),
        "public.users"
    );
    assert_eq!(
        SqlParser::normalize_identifier("`app`.`orders`"),
        "app.orders"
    );
    assert_eq!(
        SqlParser::normalize_identifier("[dbo].[customers];"),
        "dbo.customers"
    );
    assert_eq!(
        SqlParser::identifier_qualifier(r#""public"."calculate_score""#).as_deref(),
        Some("public")
    );
    assert_eq!(
        SqlParser::identifier_qualifier("catalog.public.calculate_score").as_deref(),
        Some("catalog.public")
    );
    assert_eq!(SqlParser::identifier_qualifier("calculate_score"), None);
}

#[test]
fn normalizes_sqlserver_catalog_and_default_schema_paths() {
    assert_eq!(
        SqlParser::normalize_identifier("[BarDB]..[orders]"),
        "BarDB.dbo.orders"
    );
    assert!(SqlParser::table_name_matches_with_catalog(
        "[ServerOne].[AppDb].[dbo].[Users]",
        Some("AppDb"),
        "dbo",
        "Users",
    ));
    assert!(SqlParser::table_name_matches_with_catalog(
        "AppDb..Users",
        Some("AppDb"),
        "dbo",
        "Users",
    ));
    assert!(!SqlParser::table_name_matches_with_catalog(
        "[ServerOne].[OtherDb].[dbo].[Users]",
        Some("AppDb"),
        "dbo",
        "Users",
    ));

    let sql = "SELECT * FROM [ServerOne].[AppDb].[dbo].[Users] u JOIN BarDB..Orders o ON o.user_id = u.id";
    let mut parser = SqlParser::new();
    let tree = parser.parse(sql).tree.expect("SQL should parse");
    assert_eq!(
        parser.extract_referenced_tables(&tree, sql),
        vec![
            "ServerOne.AppDb.dbo.Users".to_string(),
            "BarDB.dbo.Orders".to_string(),
        ]
    );
    assert_eq!(
        SqlParser::column_qualifier_before_position(
            "SELECT * FROM BarDB..",
            position_at_end("SELECT * FROM BarDB.."),
        )
        .as_deref(),
        Some("BarDB.dbo")
    );
}

#[test]
fn extracts_column_qualifier_before_cursor() {
    let alias_dot = "SELECT * FROM users u WHERE u.";
    assert_eq!(
        SqlParser::column_qualifier_before_position(alias_dot, position_at_end(alias_dot))
            .as_deref(),
        Some("u")
    );

    let alias_prefix = "SELECT * FROM users u WHERE u.na";
    assert_eq!(
        SqlParser::column_qualifier_before_position(alias_prefix, position_at_end(alias_prefix))
            .as_deref(),
        Some("u")
    );

    let qualified_dot = "SELECT * FROM public.users.";
    assert_eq!(
        SqlParser::column_qualifier_before_position(qualified_dot, position_at_end(qualified_dot))
            .as_deref(),
        Some("public.users")
    );

    let no_qualifier = "SELECT name FROM users";
    assert_eq!(
        SqlParser::column_qualifier_before_position(no_qualifier, position_at_end(no_qualifier)),
        None
    );
}

#[test]
fn matches_schema_qualified_table_names() {
    assert!(SqlParser::table_name_matches("users", "public", "users"));
    assert!(SqlParser::table_name_matches(
        r#""public"."users""#,
        "public",
        "users"
    ));
    assert!(SqlParser::table_name_matches(
        "`app`.`orders`",
        "app",
        "orders"
    ));
    assert!(!SqlParser::table_name_matches(
        "archive.users",
        "public",
        "users"
    ));
}
