//! Recognize complete array forms for PostgreSQL dialect diagnostics.
//! Array dimensions are owned by the grammar, never recovered as subscripts.
use super::types::SqlParser;
use tree_sitter::Node;

impl SqlParser {
    pub(super) fn unsupported_postgres_array_constructor(node: Node<'_>) -> bool {
        if node.kind() != "array" {
            return false;
        }
        let mut has_keyword = false;
        let mut has_query = false;
        for child in node.named_children(&mut node.walk()) {
            has_keyword |= child.kind() == "keyword_array";
            has_query |= matches!(
                child.kind(),
                "select" | "set_operation" | "cte" | "subquery"
            );
        }
        let brackets = node
            .children(&mut node.walk())
            .any(|child| child.kind() == "[");
        !has_keyword || !brackets && !has_query
    }

    pub(crate) fn is_postfix_array_cast(node: Node<'_>) -> bool {
        node.kind() == "cast"
            && node.child_by_field_name("parameter").is_none()
            && node
                .children(&mut node.walk())
                .any(|child| child.kind() == "::")
            && node
                .named_children(&mut node.walk())
                .any(|child| child.kind() == "array_size_definition")
    }
}
