use super::super::normalization::normalize_for_parser;
use super::*;
use crate::placeholder::SqlPlaceholderDialect;
#[test]
fn brackets_are_dialect_gated_and_preserve_protected_regions() {
    let sql = "SELECT [u].[odd]]name], '[leave]]literal]' /* [leave comment] */ FROM [users] [u]";
    let adapted = normalize_for_parser(sql, SqlPlaceholderDialect::SqlServer);
    assert_eq!(
        adapted,
        "SELECT p__.p__________, '[leave]]literal]' /* [leave comment] */ FROM p______ p__"
    );
    assert_eq!(adapted.len(), sql.len());
    for dialect in [
        SqlPlaceholderDialect::Postgres,
        SqlPlaceholderDialect::Mysql,
        SqlPlaceholderDialect::Generic,
    ] {
        let sql = "SELECT arr[1], '[]' FROM users";
        assert_eq!(normalize_for_parser(sql, dialect), sql);
    }
}
#[test]
fn bracket_tree_retains_unicode_original_text_and_ranges() {
    let sql = "/* 😀 */ SELECT [u].[列]]名] FROM [users] [u]";
    let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::SqlServer);
    let parsed = parser.parse(sql);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let offset = sql.find("名").unwrap();
    let node = parser
        .get_node_at_position(
            parsed.tree.as_ref().unwrap(),
            crate::position::byte_position_at_end(&sql[..offset]),
        )
        .unwrap();
    assert_eq!(parser.node_text(node, sql), "[列]]名]");
    assert_eq!(
        SqlParser::identifier_last_part(&parser.node_text(node, sql)),
        "列]名"
    );
    let range = parser.node_range(node);
    assert_eq!(
        range.start,
        crate::position::lsp_position_at_end(&sql[..sql.find("[列").unwrap()])
    );
    assert_eq!(
        range.end,
        crate::position::lsp_position_at_end(&sql[..sql.find(" FROM").unwrap()])
    );
}
#[test]
fn bracket_adaptation_keeps_real_and_unclosed_errors() {
    let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::SqlServer);
    for sql in [
        "SELECT (1 + ) FROM [users]",
        "SELECT [unclosed FROM users",
        "SELECT FROM [users]",
    ] {
        assert!(!parser.parse(sql).diagnostics.is_empty(), "{sql}");
    }
}
#[test]
fn bracket_markers_inside_nested_comments_are_not_adapted() {
    let sql = "SELECT [name] /* outer /* [x] */ [y] */ FROM [users]";
    let adapted = normalize_for_parser(sql, SqlPlaceholderDialect::SqlServer);
    assert!(adapted.contains("/* outer /* [x] */ [y] */"));
    assert!(adapted.starts_with("SELECT p_____"));
}
