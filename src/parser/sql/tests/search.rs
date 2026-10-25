use super::*;
use crate::placeholder::SqlPlaceholderDialect;

#[test]
fn fulltext_modes_preserve_bytes_fields_and_line_breaks() {
    for mode in [
        "",
        " IN BOOLEAN MODE",
        " IN NATURAL LANGUAGE MODE",
        " WITH QUERY EXPANSION",
        " IN NATURAL LANGUAGE MODE WITH QUERY EXPANSION",
    ] {
        let sql = format!("/* 🧭 */ SELECT MATCH(u.`名称`, /* field */ u.email)\r\nAGAINST (CONCAT(:query,' oxide'){mode}) AS rank FROM users u");
        let normalized =
            super::super::normalization::normalize_for_parser(&sql, SqlPlaceholderDialect::Mysql);
        assert_eq!(normalized.len(), sql.len());
        assert_eq!(
            normalized
                .match_indices('\n')
                .map(|(i, _)| i)
                .collect::<Vec<_>>(),
            sql.match_indices('\n').map(|(i, _)| i).collect::<Vec<_>>()
        );
        assert!(sql.contains("u.`名称`"));
        let spans = SqlParser::match_against_expressions(&sql);
        assert_eq!(spans.len(), 1, "{sql}");
        assert!(sql[spans[0].start..spans[0].end].starts_with("MATCH"));
        let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::Mysql);
        let parsed = parser.parse(&sql);
        assert!(
            parsed.diagnostics.is_empty(),
            "{sql}: {:?}",
            parsed.diagnostics
        );
        let tokens = parser.tokenize(parsed.tree.as_ref().unwrap(), &sql);
        assert!(tokens.iter().any(|t| t.text == "`名称`"), "{tokens:?}");
        let token = tokens
            .iter()
            .find(|t| t.text == "email")
            .expect("field token");
        assert_eq!(
            token.position,
            crate::position::lsp_position_at_end(&sql[..sql.find("email").unwrap()])
        );
    }
}

#[test]
fn fulltext_recognizer_rejects_noncolumns_and_protects_literal_udf_names() {
    for sql in [
        "SELECT 'MATCH(x) AGAINST (y IN BOOLEAN MODE)'",
        "SELECT app.MATCH(x) AGAINST ('q')",
        "SELECT MATCH() AGAINST ('q')",
        "SELECT MATCH(LOWER(x)) AGAINST ('q')",
        "SELECT MATCH('x') AGAINST ('q')",
        "SELECT MATCH(x,) AGAINST ('q')",
        "SELECT MATCH(x) AGAINST ()",
        "SELECT MATCH(x) AGAINST (IN BOOLEAN MODE)",
        "SELECT MATCH(x) AGAINST ('q','r')",
        "SELECT MATCH(x) AGAINST ('q'",
        "/* MATCH(x) AGAINST ('q') */ SELECT 1",
    ] {
        assert!(
            SqlParser::match_against_expressions(sql).is_empty(),
            "{sql}"
        );
    }
}

#[test]
fn fulltext_incremental_parse_matches_fresh_after_mode_and_field_edits() {
    let old = "SELECT MATCH(u.name) AGAINST (:query IN BOOLEAN MODE) FROM users u";
    let new =
        "/* 🧭 */ SELECT MATCH(u.name,u.email) AGAINST (:query WITH QUERY EXPANSION) FROM users u";
    let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::Mysql);
    let first = parser.parse(old);
    let edited = parser.parse_incremental(new, old, first.tree.as_ref().unwrap());
    let mut fresh = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::Mysql);
    let expected = fresh.parse(new);
    assert_eq!(
        edited.tree.unwrap().root_node().to_sexp(),
        expected.tree.unwrap().root_node().to_sexp()
    );
    assert_eq!(edited.diagnostics, expected.diagnostics);
    assert!(edited.diagnostics.is_empty());
}

#[test]
fn search_adapters_preserve_operators_and_protected_identifier_text() {
    let sql = "/* 🧭 */ SELECT to_tsquery('a')&&to_tsquery('b'), 'a&&b'";
    let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::Postgres);
    let parsed = parser.parse(sql);
    assert!(parsed.diagnostics.is_empty());
    let tree = parsed.tree.unwrap();
    let tokens = parser.tokenize(&tree, sql);
    let token = tokens
        .iter()
        .find(|token| token.text == "&&")
        .expect("original compound operator");
    assert_eq!(
        token.position,
        crate::position::lsp_position_at_end(&sql[..sql.find("&&").unwrap()])
    );
    let node = tree
        .root_node()
        .descendant_for_byte_range(sql.find("&&").unwrap(), sql.find("&&").unwrap())
        .unwrap();
    let range = parser.node_range(node);
    assert_eq!(range.end.character - range.start.character, 2);
    let protected = "SELECT '`名称`', `列``名称`, u.`名称` FROM users u -- `注释`";
    let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::Mysql);
    let parsed = parser.parse(protected);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let tokens = parser.tokenize(parsed.tree.as_ref().unwrap(), protected);
    assert!(tokens.iter().any(|t| t.text == "`列``名称`"));
    assert!(tokens.iter().any(|t| t.text == "'`名称`'"));
}

#[test]
fn vendor_search_queries_have_no_spurious_syntax_diagnostics() {
    let mut parser = SqlParser::new();
    for sql in [
        "SELECT ft.RANK FROM CONTAINSTABLE(users,(name,email),N'NEAR((oxide,studio),5,TRUE)',LANGUAGE 1033,25) ft",
        "SELECT u.id FROM users u WHERE CONTAINS((u.name,u.email), N'FORMSOF(INFLECTIONAL, run)', LANGUAGE 1033)",
        "SELECT u.id FROM users u WHERE FREETEXT(u.name, N'oxide studio', LANGUAGE N'English')",
        "SELECT SCORE(7) AS relevance FROM users u WHERE CONTAINS(u.name,'NEAR((oxide,studio),5)',7)>0",
    ] {
        let parsed=parser.parse(sql);
        assert!(parsed.diagnostics.is_empty(),"{sql}: {:?}",parsed.diagnostics);
    }
}

#[test]
fn fulltext_language_adaptation_keeps_protected_regions_and_incremental_offsets() {
    for sql in [
        "SELECT app.CONTAINS(x,y,LANGUAGE 1033)",
        "SELECT user_fn(x,y,LANGUAGE 1033)",
        "SELECT CONTAINS(x,LANGUAGE 1033,y)",
        "SELECT 'CONTAINS(x,y,LANGUAGE 1033)'",
        "/* CONTAINS(x,y,LANGUAGE 1033) */ SELECT 1",
        "SELECT CONTAINS(x,y,LANGUAGE_VALUE)",
    ] {
        assert_eq!(
            super::super::normalization::normalize_for_parser(sql, SqlPlaceholderDialect::Generic),
            sql,
            "{sql}"
        );
    }
    let old = "/* 🧭 */ SELECT ft.RANK FROM CONTAINSTABLE(users,name,N'oxide',LANGUAGE 1033,25) ft";
    let new="/* 🧭 */ SELECT ft.RANK FROM CONTAINSTABLE(users,name,N'oxide',LANGUAGE\r\nN'English',25) ft";
    let normalized =
        super::super::normalization::normalize_for_parser(new, SqlPlaceholderDialect::Generic);
    assert_eq!(normalized.len(), new.len());
    assert_eq!(normalized.find("N'English'"), new.find("N'English'"));
    assert_eq!(normalized.find("\r\n"), new.find("\r\n"));
    let mut parser = SqlParser::new();
    let initial = parser.parse(old);
    let edited = parser.parse_incremental(new, old, initial.tree.as_ref().unwrap());
    let fresh = SqlParser::new().parse(new);
    assert_eq!(edited.diagnostics, fresh.diagnostics);
    assert!(edited.diagnostics.is_empty());
    assert_eq!(
        edited.tree.unwrap().root_node().to_sexp(),
        fresh.tree.unwrap().root_node().to_sexp()
    );
    assert!(!parser
        .parse("SELECT ft.RANK FROM CONTAINSTABLE(users,name,N'oxide',LANGUAGE (1+),25) ft")
        .diagnostics
        .is_empty());
}
