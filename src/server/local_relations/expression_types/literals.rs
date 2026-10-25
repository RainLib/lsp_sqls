//! PostgreSQL string constants acquire their type from expression context.
//! Keep that distinct from a text-typed scalar query or an explicit cast.
use super::{ExpressionType, UNTYPED_STRING};
pub(super) fn operand(mut node: tree_sitter::Node<'_>) -> tree_sitter::Node<'_> {
    for _ in 0..32 {
        let next = match node.kind() {
            "term" => node.child_by_field_name("value"),
            "parenthesized_expression" => node
                .named_children(&mut node.walk())
                .find(|child| !child.is_extra()),
            _ => None,
        };
        let Some(next) = next else {
            break;
        };
        node = next;
    }
    node
}
pub(super) fn string_literal(node: tree_sitter::Node<'_>, sql: &str, dialect: &str) -> bool {
    let node = operand(node);
    node.kind() == "literal"
        && node.utf8_text(sql.as_bytes()).is_ok_and(|text| {
            text.starts_with('\'')
                || matches!(dialect, "postgres" | "duckdb")
                    && (text.starts_with("E'") || text.starts_with("e'") || text.starts_with('$'))
        })
}
pub(super) fn infer(text: &str, dialect: &str) -> Option<ExpressionType> {
    if matches!(dialect, "duckdb" | "clickhouse" | "sqlite") && text.eq_ignore_ascii_case("NULL") {
        return Some(ExpressionType::new("null", true));
    }
    if dialect == "hive" {
        if text.starts_with('\'') && text.ends_with('\'') && text.len() >= 2 {
            return Some(ExpressionType::new("string", false));
        }
        if text.eq_ignore_ascii_case("NULL") {
            return Some(ExpressionType::new("null", true));
        }
        if ["true", "false"]
            .iter()
            .any(|v| text.eq_ignore_ascii_case(v))
        {
            return Some(ExpressionType::new("boolean", false));
        }
        if text.parse::<i32>().is_ok() {
            return Some(ExpressionType::new("int", false));
        }
        if text.parse::<i64>().is_ok() {
            return Some(ExpressionType::new("bigint", false));
        }
    }
    if dialect == "postgres" && text.starts_with('\'') && text.ends_with('\'') && text.len() >= 2 {
        return Some(ExpressionType::new(UNTYPED_STRING, false));
    }
    super::arithmetic::numeric_literal(text, dialect)
}
