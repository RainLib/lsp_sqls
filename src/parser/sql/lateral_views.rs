//! Hive table-generating functions retain declaration and input coordinates.
use super::types::SqlParser;
use std::ops::Range;
use tree_sitter::Tree;

pub(crate) struct LateralView {
    pub range: Range<usize>,
    pub from: Range<usize>,
    pub function: String,
    pub arguments: Vec<Range<usize>>,
    pub alias: String,
    pub columns: Vec<String>,
    pub outer: bool,
}
impl SqlParser {
    pub(crate) fn lateral_views(tree: &Tree, source: &str) -> Vec<LateralView> {
        let mut cursor = tree.walk();
        let mut result = Vec::new();
        let mut budget = 32768;
        'walk: loop {
            if budget == 0 || result.len() > 64 {
                return Vec::new();
            }
            budget -= 1;
            let node = cursor.node();
            if node.kind() == "lateral_view" && !node.has_error() {
                if let (Some(function), Some(alias)) = (
                    node.child_by_field_name("function"),
                    node.child_by_field_name("alias"),
                ) {
                    if let Some(reference) = function
                        .named_children(&mut function.walk())
                        .find(|n| n.kind() == "object_reference")
                    {
                        let mut parent = node.parent();
                        while parent.is_some_and(|n| n.kind() != "from") {
                            parent = parent.and_then(|n| n.parent());
                        }
                        if let Some(from) = parent {
                            result.push(LateralView {
                                range: node.byte_range(),
                                from: from.byte_range(),
                                function: source[reference.byte_range()].into(),
                                arguments: function
                                    .named_children(&mut function.walk())
                                    .filter(|n| n.kind() == "term")
                                    .map(|n| n.byte_range())
                                    .collect(),
                                alias: Self::identifier_last_part(&source[alias.byte_range()]),
                                columns: node
                                    .children_by_field_name("column", &mut node.walk())
                                    .map(|n| Self::identifier_last_part(&source[n.byte_range()]))
                                    .collect(),
                                outer: node
                                    .named_children(&mut node.walk())
                                    .any(|n| n.kind() == "keyword_outer"),
                            });
                        }
                    }
                }
            }
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    break 'walk;
                }
            }
        }
        result.sort_by_key(|view| view.range.start);
        result
    }
}
