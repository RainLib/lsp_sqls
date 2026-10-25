//! Keep component types local to row predicates. A row never becomes a fake
//! scalar type or leaks a record marker into completion metadata.
use super::{predicates, ExpressionType, MAX_COMPOUND_ITEMS};
use crate::server::local_relations::projection::ProjectionSources;
use tree_sitter::Node;

pub(super) fn is_row(
    mut node: Node<'_>,
    sql: &str,
    sources: &ProjectionSources<'_>,
    dialect: &str,
) -> bool {
    for _ in 0..32 {
        if node.kind() != "parenthesized_expression" {
            break;
        }
        let Some(inner) = node.named_child(0) else {
            return false;
        };
        node = inner;
    }
    match node.kind() {
        "list" => true,
        "invocation" => constructor(node, sql, dialect),
        "subquery" => sources
            .row_types(node.start_byte(), node.end_byte())
            .is_some_and(|columns| columns.len() > 1),
        _ => false,
    }
}

fn constructor(node: Node<'_>, sql: &str, dialect: &str) -> bool {
    matches!(dialect, "postgres" | "mysql")
        && node
            .named_children(&mut node.walk())
            .find(|child| child.kind() == "object_reference")
            .and_then(|name| name.utf8_text(sql.as_bytes()).ok())
            .is_some_and(|name| name.eq_ignore_ascii_case("ROW"))
}

fn components(
    node: Node<'_>,
    sql: &str,
    sources: &ProjectionSources<'_>,
    dialect: &str,
    depth: usize,
) -> Option<Vec<ExpressionType>> {
    if depth >= 32 {
        return None;
    }
    if node.kind() == "parenthesized_expression" {
        return components(node.named_child(0)?, sql, sources, dialect, depth + 1);
    }
    if node.kind() == "subquery" {
        return sources
            .row_types(node.start_byte(), node.end_byte())
            .map(<[ExpressionType]>::to_vec);
    }
    let invocation = node.kind() == "invocation" && constructor(node, sql, dialect);
    if node.kind() != "list" && !invocation {
        return None;
    }
    let mut values = Vec::new();
    for child in node
        .named_children(&mut node.walk())
        .filter(|child| !child.is_extra())
    {
        if invocation && child.kind() != "term" {
            continue;
        }
        if values.len() == MAX_COMPOUND_ITEMS {
            return None;
        }
        let value = if child.kind() == "term" {
            child.child_by_field_name("value")?
        } else {
            child
        };
        values.push(
            super::infer(value, sql, sources, Some(dialect), depth + 1).unwrap_or_else(|| {
                ExpressionType::new("unknown", predicates::nullable(None, value, sql))
            }),
        );
    }
    let minimum = if invocation && dialect == "postgres" {
        1
    } else {
        2
    };
    (values.len() >= minimum).then_some(values)
}

pub(super) fn compare(
    left: Node<'_>,
    right: Node<'_>,
    sql: &str,
    sources: &ProjectionSources<'_>,
    dialect: &str,
    depth: usize,
) -> Option<ExpressionType> {
    let left = components(left, sql, sources, dialect, depth)?;
    let right = components(right, sql, sources, dialect, depth)?;
    if left.len() != right.len()
        || dialect == "postgres"
            && !left
                .iter()
                .zip(&right)
                .all(|(left, right)| predicates::pg_comparable(Some(left), Some(right)))
    {
        return None;
    }
    predicates::result(dialect, left.iter().chain(&right).any(|ty| ty.nullable))
}
