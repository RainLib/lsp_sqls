//! The shared grammar is a superset; QUALIFY legality stays profile-specific.
use crate::{parser::SqlParser, position::SourcePositions};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Range};
use tree_sitter::{Node, Tree};

pub(super) fn diagnostics(text: &str, profile: &str, tree: Option<&Tree>) -> Vec<Diagnostic> {
    if !matches!(
        profile,
        "clickhouse" | "postgres" | "mysql" | "sqlserver" | "sqlite" | "hive"
    ) || text.len() > 512 * 1024
        || !text
            .as_bytes()
            .windows(7)
            .any(|word| matches!(word[0], b'Q' | b'q') && word.eq_ignore_ascii_case(b"QUALIFY"))
    {
        return Vec::new();
    }
    let fallback = tree
        .is_none()
        .then(|| SqlParser::new().parse_syntax_tree(text))
        .flatten();
    let Some(tree) = tree.or(fallback.as_ref()) else {
        return Vec::new();
    };
    let positions = SourcePositions::new(text);
    let at = |offset| {
        positions.as_ref().map_or_else(
            || crate::position::lsp_position_at_end(&text[..offset]),
            |p| p.offset(text, offset),
        )
    };
    let mut result = Vec::new();
    let mut walk = tree.walk();
    let mut remaining = 32768;
    'walk: loop {
        if remaining == 0 {
            break;
        }
        remaining -= 1;
        let node = walk.node();
        if node.kind() == "qualify" {
            let message = if profile != "clickhouse" {
                Some(format!("QUALIFY is not supported by the {profile} profile"))
            } else if node.parent().and_then(own_window) == Some(false) {
                Some("QUALIFY requires a window function in the same SELECT query".into())
            } else {
                None
            };
            if let Some(message) = message {
                result.push(Diagnostic {
                    range: Range::new(at(node.start_byte()), at(node.start_byte() + 7)),
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: Some(NumberOrString::String(
                        if profile == "clickhouse" {
                            "QUALIFY_REQUIRES_WINDOW"
                        } else {
                            "UNSUPPORTED_DIALECT_SYNTAX"
                        }
                        .into(),
                    )),
                    source: Some("sql-dialect".into()),
                    message,
                    ..Default::default()
                });
            }
        }
        if walk.goto_first_child() {
            continue;
        }
        loop {
            if walk.goto_next_sibling() {
                break;
            }
            if !walk.goto_parent() {
                break 'walk;
            }
        }
    }
    result
}

fn own_window(owner: Node<'_>) -> Option<bool> {
    let mut pending = vec![owner];
    let mut remaining = 8192;
    while let Some(node) = pending.pop() {
        if remaining == 0 {
            return None;
        }
        remaining -= 1;
        if node.kind() == "window_function" {
            return Some(true);
        }
        if node.id() != owner.id()
            && matches!(
                node.kind(),
                "subquery" | "cte" | "set_operation" | "set_operand" | "statement"
            )
        {
            continue;
        }
        pending.extend(node.named_children(&mut node.walk()));
    }
    Some(false)
}
