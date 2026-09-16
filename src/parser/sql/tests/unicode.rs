use super::*;

#[test]
fn converts_lsp_utf16_position_to_tree_sitter_byte_position() {
    let sql = "SELECT '😀' FROM users WHERE ";
    let lsp_position = Position {
        line: 0,
        character: sql.encode_utf16().count() as u32,
    };

    assert_eq!(
        SqlParser::lsp_position_to_byte_position(sql, lsp_position),
        Position {
            line: 0,
            character: sql.len() as u32,
        }
    );

    let inside_surrogate_pair = Position {
        line: 0,
        character: "SELECT '😀".encode_utf16().count() as u32 - 1,
    };
    let byte_position = SqlParser::lsp_position_to_byte_position(sql, inside_surrogate_pair);
    assert_eq!(byte_position.character as usize, "SELECT '".len());
}

#[test]
fn reports_multiline_non_ascii_diagnostics_in_utf16_columns() {
    let invalid_line = "SELECT '😀中文' AS label FROM users WHERE id = )";
    let sql = format!("SELECT 1;\n{invalid_line}");
    let error_byte_column = invalid_line.rfind("= )").expect("error marker");
    let expected_start = Position {
        line: 1,
        character: invalid_line[..error_byte_column].encode_utf16().count() as u32,
    };

    let mut parser = SqlParser::new();
    let result = parser.parse(&sql);
    let diagnostic = result
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.range.start.line == 1 && diagnostic.message.contains(')'))
        .unwrap_or_else(|| panic!("expected a diagnostic for ')': {:?}", result.diagnostics));

    assert_eq!(diagnostic.range.start, expected_start);
    assert_eq!(
        diagnostic.range.end,
        Position {
            line: 1,
            character: invalid_line.encode_utf16().count() as u32,
        }
    );
    assert_ne!(
        diagnostic.range.start.character, error_byte_column as u32,
        "the fixture must catch a byte-column implementation"
    );
}

#[test]
fn exposes_node_and_token_positions_as_utf16_after_non_ascii_text() {
    let target_line = "SELECT '😀中文' AS label FROM users;";
    let sql = format!("SELECT 1;\n{target_line}");
    let target_byte_column = target_line.find("users").expect("users column");
    let target_byte_offset = sql.find("users").expect("users offset");
    let expected_start = Position {
        line: 1,
        character: target_line[..target_byte_column].encode_utf16().count() as u32,
    };

    let mut parser = SqlParser::new();
    let result = parser.parse(&sql);
    let tree = result.tree.as_ref().expect("SQL should produce a tree");
    let node = tree
        .root_node()
        .descendant_for_byte_range(target_byte_offset, target_byte_offset + "users".len())
        .expect("users node");

    assert_eq!(parser.node_text(node, &sql), "users");
    assert_eq!(
        parser.node_range(node),
        Range {
            start: expected_start,
            end: Position {
                line: 1,
                character: expected_start.character + "users".encode_utf16().count() as u32,
            },
        }
    );
    assert!(parser
        .tokenize(tree, &sql)
        .iter()
        .any(|token| { token.text == "users" && token.position == expected_start }));
}

#[test]
fn suppresses_diagnostics_for_interactive_trailing_sql() {
    let samples = [
        "SELECT",
        "SELECT * FROM",
        "SELECT * FROM public.users WHERE",
        "SELECT * FROM public.users WHERE id =",
        "SELECT id,",
        "SELECT * FROM public.",
    ];

    for sql in samples {
        let mut parser = SqlParser::new();
        let result = parser.parse(sql);
        assert!(
            result.diagnostics.is_empty(),
            "interactive SQL should not report diagnostics for {sql:?}: {:?}",
            result.diagnostics
        );
    }
}

#[test]
fn keeps_diagnostics_for_closed_invalid_sql() {
    let mut parser = SqlParser::new();
    let result = parser.parse("SELECT * FROM users WHERE id = )");

    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Some(DiagnosticSeverity::ERROR)),
        "closed invalid SQL should retain an error diagnostic: {:?}",
        result.diagnostics
    );
}
