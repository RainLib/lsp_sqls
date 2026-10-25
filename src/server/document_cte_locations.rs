//! CTE identity belongs to one WITH owner, including unaliased qualifiers.
use super::document_structure::range_for_offsets;
use super::locations::range_contains_position;
use crate::parser::SqlParser;
use std::collections::HashMap;
use tower_lsp::lsp_types::{Location, Position, Range, Url};
use tree_sitter::Node;
pub(super) struct CteResolution {
    pub(super) definition: Range,
    pub(super) selected: Range,
    pub(super) locations: Vec<Location>,
}
fn same_name(left: &str, right: &str, dialect: &str) -> bool {
    let key = |raw: &str| {
        let name = SqlParser::identifier_last_part(raw);
        if dialect == "postgres" && raw.starts_with('"') {
            name
        } else {
            name.to_ascii_lowercase()
        }
    };
    key(left) == key(right)
}
fn owner(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node.parent();
    while let Some(node) = current {
        if matches!(node.kind(), "statement" | "subquery") {
            return Some(node);
        }
        current = node.parent();
    }
    None
}
fn declaration<'a>(reference: Node<'a>, source: &str, dialect: &str) -> Option<Node<'a>> {
    let name = &source[reference.byte_range()];
    if SqlParser::identifier_parts(name).len() != 1 {
        return None;
    }
    let mut current = owner(reference);
    while let Some(scope) = current {
        let recursive = dialect == "sqlserver"
            || scope
                .named_children(&mut scope.walk())
                .any(|node| node.kind() == "keyword_recursive");
        let mut cursor = scope.walk();
        let mut matches = scope
            .named_children(&mut cursor)
            .filter(|node| {
                node.kind() == "cte" && (recursive || node.end_byte() <= reference.start_byte())
            })
            .filter_map(|node| node.named_child(0))
            .filter(|node| same_name(&source[node.byte_range()], name, dialect));
        if let Some(found) = matches.next() {
            return matches.next().is_none().then_some(found);
        }
        current = owner(scope);
    }
    None
}
fn qualifier_declaration<'a>(
    reference: Node<'a>,
    relations: &HashMap<usize, Vec<Node<'a>>>,
    source: &str,
    dialect: &str,
) -> Option<Node<'a>> {
    let name = &source[reference.byte_range()];
    if SqlParser::identifier_parts(name).len() != 1 {
        return None;
    }
    let mut current = owner(reference);
    while let Some(scope) = current {
        let rows = relations
            .get(&scope.id())
            .map(Vec::as_slice)
            .unwrap_or_default();
        if rows.iter().any(|row| {
            row.child_by_field_name("alias")
                .is_some_and(|alias| same_name(&source[alias.byte_range()], name, dialect))
        }) {
            return None;
        }
        let mut matched = rows
            .iter()
            .copied()
            .filter(|row| row.child_by_field_name("alias").is_none())
            .filter_map(|row| row.named_child(0))
            .filter(|value| {
                value.kind() == "object_reference"
                    && SqlParser::identifier_parts(&source[value.byte_range()]).len() == 1
                    && same_name(&source[value.byte_range()], name, dialect)
            });
        if let Some(row) = matched.next() {
            return matched
                .next()
                .is_none()
                .then(|| declaration(row, source, dialect))
                .flatten();
        }
        current = owner(scope);
    }
    None
}
pub(super) fn resolve_cte(
    source: &str,
    uri: &Url,
    position: Position,
    dialect: &str,
    include_declaration: bool,
) -> Option<CteResolution> {
    if !matches!(
        dialect,
        "postgres" | "mysql" | "sqlite" | "duckdb" | "sqlserver" | "hive" | "clickhouse"
    ) {
        return None;
    }
    if source.len() > super::local_relations::LOCAL_RELATION_SCAN_MAX_BYTES {
        return None;
    }
    let mut parser = SqlParser::new();
    let tree = parser.parse_syntax_tree(source)?;
    let mut nodes = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if nodes.len() >= 262144 {
            return None;
        }
        nodes.push(node);
        stack.extend(node.named_children(&mut node.walk()));
    }
    let mut relations: HashMap<usize, Vec<Node<'_>>> = HashMap::new();
    for node in nodes
        .iter()
        .copied()
        .filter(|node| node.kind() == "relation")
    {
        if let Some(scope) = owner(node) {
            relations.entry(scope.id()).or_default().push(node);
        }
    }
    let mut bindings = Vec::new();
    for node in nodes {
        if node.kind() == "cte" {
            if let Some(name) = node.named_child(0) {
                bindings.push((name, name, true));
            }
        }
        if node.kind() != "object_reference" {
            continue;
        }
        let Some(parent) = node.parent() else {
            continue;
        };
        let definition = if parent.kind() == "relation" {
            declaration(node, source, dialect)
        } else if matches!(parent.kind(), "field" | "all_fields") {
            qualifier_declaration(node, &relations, source, dialect)
        } else {
            None
        };
        if let Some(definition) = definition {
            bindings.push((node, definition, false));
        }
    }
    let (selected, definition, _) = bindings.iter().find(|(node, _, _)| {
        range_contains_position(
            range_for_offsets(source, node.start_byte(), node.end_byte()),
            position,
        )
    })?;
    let locations = bindings
        .iter()
        .filter(|(_, bound, is_declaration)| {
            bound.id() == definition.id() && (include_declaration || !*is_declaration)
        })
        .map(|(node, _, _)| Location {
            uri: uri.clone(),
            range: range_for_offsets(source, node.start_byte(), node.end_byte()),
        })
        .collect();
    Some(CteResolution {
        definition: range_for_offsets(source, definition.start_byte(), definition.end_byte()),
        selected: range_for_offsets(source, selected.start_byte(), selected.end_byte()),
        locations,
    })
}
