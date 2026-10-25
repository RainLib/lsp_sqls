//! ANY/SOME/ALL produce a predicate, not the selected type of their operand.
//! Array element nullability is bounded; subquery contents stay conservative.
use super::{predicates, ExpressionType, MAX_COMPOUND_ITEMS};
use crate::server::local_relations::projection::ProjectionSources;
use tree_sitter::Node;

pub(in crate::server::local_relations) fn operand(node: Node<'_>) -> Option<Node<'_>> {
    (node.kind() == "unary_expression"
        && node
            .child_by_field_name("operator")
            .is_some_and(|operator| {
                matches!(
                    operator.kind(),
                    "keyword_any" | "keyword_some" | "keyword_all"
                )
            }))
    .then(|| node.child_by_field_name("operand"))
    .flatten()
}

pub(super) fn infer(
    left: (Option<&ExpressionType>, Node<'_>),
    quantified: Node<'_>,
    operator: &str,
    sql: &str,
    sources: &ProjectionSources<'_>,
    dialect: &str,
    depth: usize,
) -> Option<ExpressionType> {
    if !matches!(dialect, "postgres" | "mysql") {
        return None;
    }
    let mut argument = operand(quantified)?;
    let mut depth = depth + 1;
    while argument.kind() == "parenthesized_expression" {
        if depth >= 32 {
            return None;
        }
        depth += 1;
        argument = argument.named_child(0)?;
    }
    if argument.kind() == "subquery" {
        return predicates::result(dialect, true);
    }
    if dialect != "postgres" || depth >= 32 {
        return None;
    }
    let ty = super::infer(argument, sql, sources, Some(dialect), depth + 1);
    let typed_array = argument.kind() == "cast"
        && argument
            .named_children(&mut argument.walk())
            .any(|node| node.kind() == "array_size_definition")
        || ty
            .as_ref()
            .is_some_and(|ty| ty.data_type.trim_end().ends_with("[]"));
    let mut array = argument;
    loop {
        if depth >= 32 {
            return None;
        }
        let inner = if array.kind() == "parenthesized_expression" {
            array.named_child(0)
        } else if typed_array {
            (array.kind() == "cast")
                .then(|| {
                    array.child_by_field_name("parameter").or_else(|| {
                        array
                            .named_children(&mut array.walk())
                            .find(|child| !child.is_extra())
                    })
                })
                .flatten()
        } else {
            None
        };
        let Some(inner) = inner else {
            break;
        };
        array = inner;
        depth += 1;
    }
    if array.kind() != "array" {
        return typed_array
            .then(|| predicates::result(dialect, true))
            .flatten();
    }
    let mut count = 0;
    let mut nullable = predicates::nullable(left.0, left.1, sql);
    for child in array
        .named_children(&mut array.walk())
        .filter(|node| !node.is_extra() && node.kind() != "keyword_array")
    {
        // ARRAY(SELECT ...) has query children rather than literal elements.
        if matches!(child.kind(), "select" | "set_operation" | "cte") {
            return predicates::result(dialect, true);
        }
        if count == MAX_COMPOUND_ITEMS {
            return predicates::result(dialect, true);
        }
        count += 1;
        let ty = super::infer(child, sql, sources, Some(dialect), depth + 1);
        let strict_builtin = match operator {
            "=" | "<>" | "!=" | "<" | ">" | "<=" | ">=" => {
                predicates::pg_comparable(left.0, ty.as_ref())
            }
            "LIKE" | "NOT LIKE" | "ILIKE" | "NOT ILIKE" | "~" | "~*" | "!~" | "!~*" => {
                predicates::pg_text(left.0) && predicates::pg_text(ty.as_ref())
            }
            _ => false,
        };
        nullable |= predicates::nullable(ty.as_ref(), child, sql) || !strict_builtin;
    }
    if count == 0 {
        // An empty ARRAY needs an explicit element type. Both quantifiers
        // produce a non-NULL result for an empty array, even with a NULL lhs.
        return typed_array
            .then(|| predicates::result(dialect, false))
            .flatten();
    }
    predicates::result(dialect, nullable)
}
