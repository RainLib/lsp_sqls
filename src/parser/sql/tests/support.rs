use super::*;

pub(super) fn position_at_end(source: &str) -> Position {
    let mut line = 0;
    let mut character = 0;

    for ch in source.chars() {
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf8() as u32;
        }
    }

    Position { line, character }
}

pub(super) fn analyzed_context_at_end(sql: &str) -> CompletionContext {
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    let tree = result.tree.as_ref().expect("SQL should produce a tree");
    let position = position_at_end(sql);
    let node = parser
        .get_node_at_position(tree, position)
        .expect("cursor should map to an AST node");

    parser.analyze_completion_context(node, sql, position)
}
