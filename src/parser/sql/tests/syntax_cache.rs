use super::*;
use crate::parser::sql::syntax_tree::{SYNTAX_CACHE_MAX_BYTES, SYNTAX_CACHE_MAX_NODES};
use crate::placeholder::SqlPlaceholderDialect;

#[test]
fn same_text_reuses_syntax_while_cursor_positions_remain_independent() {
    let sql = "SELECT '雪😀' AS label,u.id FROM users u";
    let mut parser = SqlParser::new();
    let first = parser.parse_syntax_tree(sql).unwrap();
    let second = parser.parse_syntax_tree(sql).unwrap();
    assert_eq!(
        parser.syntax_parse_count, 1,
        "same SQL must not invoke the underlying parser twice"
    );
    assert_eq!(first.root_node().to_sexp(), second.root_node().to_sexp());
    for word in ["id", "users"] {
        let offset = sql.find(word).unwrap() + 1;
        let position = crate::position::byte_position_at_end(&sql[..offset]);
        let node = parser.get_node_at_position(&second, position).unwrap();
        assert_eq!(parser.node_text(node, sql), word);
    }
}

#[test]
fn cached_edits_match_fresh_parse_for_unicode_and_placeholders() {
    for (dialect, original, edited) in [
        (
            SqlPlaceholderDialect::Generic,
            "SELECT id FROM users WHERE id=1",
            "SELECT '雪😀' AS label,name FROM users WHERE id=2",
        ),
        (
            SqlPlaceholderDialect::Postgres,
            "SELECT id FROM users WHERE id=$1",
            "SELECT '雪😀',name FROM users WHERE id=$1 AND active=$2",
        ),
        (
            SqlPlaceholderDialect::Mysql,
            "SELECT id FROM users WHERE id=?",
            "SELECT '雪😀',name FROM users WHERE id=? AND active=?",
        ),
    ] {
        let mut parser = SqlParser::new_with_placeholder_dialect(dialect);
        parser.parse_syntax_tree(original).unwrap();
        let incremental = parser.parse_syntax_tree(edited).unwrap();
        let mut fresh = SqlParser::new_with_placeholder_dialect(dialect);
        let expected = fresh.parse(edited).tree.unwrap();
        assert_eq!(
            incremental.root_node().to_sexp(),
            expected.root_node().to_sexp()
        );
        assert_eq!(incremental.root_node().end_byte(), edited.len());
        assert_eq!(
            parser.extract_referenced_tables(&incremental, edited),
            vec!["users"]
        );
    }
}

#[test]
fn diagnostics_are_recomputed_after_tree_only_completion_and_edits() {
    let invalid = "SELECT * FROM users WHERE id = )";
    let valid = "SELECT * FROM users WHERE id = 1";
    let mut parser = SqlParser::new();
    parser.parse_syntax_tree(invalid).unwrap();
    let report = parser.parse(invalid);
    assert!(report
        .diagnostics
        .iter()
        .any(|item| item.severity == Some(DiagnosticSeverity::ERROR)));
    let clean = parser.parse(valid);
    assert!(clean.diagnostics.is_empty());
    assert_eq!(clean.source, valid);
    let again = parser.parse(invalid);
    let mut fresh = SqlParser::new();
    assert_eq!(again.diagnostics, fresh.parse(invalid).diagnostics);
}

#[test]
fn external_incremental_tree_becomes_the_current_syntax_snapshot() {
    let old = "SELECT id FROM users";
    let next = "SELECT name FROM users WHERE id=1";
    let mut parser = SqlParser::new();
    let first = parser.parse(old).tree.unwrap();
    let updated = parser.parse_incremental(next, old, &first);
    let cached = parser.parse_syntax_tree(next).unwrap();
    assert_eq!(
        parser.syntax_parse_count, 2,
        "initial + external incremental parse; cache hit adds no parse"
    );
    assert_eq!(
        updated.tree.unwrap().root_node().to_sexp(),
        cached.root_node().to_sexp()
    );
    assert_eq!(parser.source, next);
}

#[test]
fn large_source_is_parsed_without_retaining_an_extra_tree() {
    let sql = format!("SELECT 1{}", " ".repeat(SYNTAX_CACHE_MAX_BYTES));
    let mut parser = SqlParser::new();
    let tree = parser.parse_syntax_tree(&sql).unwrap();
    assert_eq!(tree.root_node().end_byte(), sql.len());
    assert!(parser.syntax_tree.is_none());
    parser.parse_syntax_tree("SELECT 2").unwrap();
    assert!(parser.syntax_tree.is_some());
    assert_eq!(parser.source, "SELECT 2");
}

#[test]
fn oversized_node_count_is_not_retained_even_for_a_small_source() {
    let sql = format!("SELECT {}1", "1,".repeat(SYNTAX_CACHE_MAX_NODES));
    assert!(sql.len() < SYNTAX_CACHE_MAX_BYTES);
    let mut parser = SqlParser::new();
    let tree = parser.parse_syntax_tree(&sql).unwrap();
    assert!(tree.root_node().descendant_count() > SYNTAX_CACHE_MAX_NODES);
    assert!(parser.syntax_tree.is_none());
}
