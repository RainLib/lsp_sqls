//! Typed PostgreSQL search operators. A familiar spelling alone is insufficient
//! to resolve operators on domains or user-defined types.
use super::{literals, numeric_base, ExpressionType};
use tree_sitter::Node;

pub(super) fn binary(
    left_operand: (Option<&ExpressionType>, Node<'_>),
    operator: &str,
    right_operand: (Option<&ExpressionType>, Node<'_>),
    sql: &str,
    dialect: &str,
    version: Option<(u32, u32)>,
) -> Option<ExpressionType> {
    if dialect != "postgres" {
        return None;
    }
    let (left, left_node) = left_operand;
    let (right, right_node) = right_operand;
    let left_type = left.map(|ty| numeric_base(&ty.data_type).to_ascii_lowercase());
    let right_type = right.map(|ty| numeric_base(&ty.data_type).to_ascii_lowercase());
    let left_type = left_type.as_deref();
    let right_type = right_type.as_deref();
    let left_string = literals::string_literal(left_node, sql, dialect);
    let right_string = literals::string_literal(right_node, sql, dialect);
    let text = |ty| {
        matches!(
            ty,
            Some("text" | "varchar" | "character varying" | "char" | "bpchar")
        )
    };
    let result = match operator.to_ascii_uppercase().as_str() {
        "AND" | "OR" if left_type == Some("boolean") && right_type == Some("boolean") => "boolean",
        "=" | "<>" | "!=" | "<" | ">" | "<=" | ">="
            if left.is_some_and(|ty| super::arithmetic::pg_rank(&ty.data_type).is_some())
                && right.is_some_and(|ty| super::arithmetic::pg_rank(&ty.data_type).is_some()) =>
        {
            "boolean"
        }
        "@@" if (matches!(left_type, Some("tsvector" | "text")) || left_string)
            && right_type == Some("tsquery") =>
        {
            "boolean"
        }
        "@@" if left_type == Some("tsquery") && right_type == Some("tsvector") => "boolean",
        "||" if left_type == Some("tsvector") && right_type == Some("tsvector") => "tsvector",
        "||" | "&&" | "<->" if left_type == Some("tsquery") && right_type == Some("tsquery") => {
            "tsquery"
        }
        "@>" | "<@" if left_type == Some("tsquery") && right_type == Some("tsquery") => "boolean",
        "->" if matches!(left_type, Some("json" | "jsonb"))
            && (right_string || matches!(right_type, Some("integer" | "int" | "text"))) =>
        {
            left_type?
        }
        "->>"
            if matches!(left_type, Some("json" | "jsonb"))
                && (right_string || matches!(right_type, Some("integer" | "int" | "text"))) =>
        {
            "text"
        }
        "#>" if matches!(left_type, Some("json" | "jsonb"))
            && (right_string
                || right_type == Some("text[]")
                || text_array_literal(right_node, sql)) =>
        {
            left_type?
        }
        "#>>"
            if matches!(left_type, Some("json" | "jsonb"))
                && (right_string
                    || right_type == Some("text[]")
                    || text_array_literal(right_node, sql)) =>
        {
            "text"
        }
        "@>" | "<@"
            if left_type == Some("jsonb") && (right_string || right_type == Some("jsonb")) =>
        {
            "boolean"
        }
        "?" if left_type == Some("jsonb") && (right_string || text(right_type)) => "boolean",
        "?|" | "?&"
            if left_type == Some("jsonb")
                && (right_string
                    || right_type == Some("text[]")
                    || text_array_literal(right_node, sql)) =>
        {
            "boolean"
        }
        "@?" | "@@"
            if left_type == Some("jsonb")
                && (right_string || right_type == Some("jsonpath"))
                && version.is_none_or(|version| version >= (12, 0)) =>
        {
            "boolean"
        }
        _ => return None,
    };
    Some(ExpressionType::new(result, true))
}

fn text_array_literal(node: Node<'_>, sql: &str) -> bool {
    if node.kind() != "array" {
        return false;
    }
    let mut count = 0;
    for child in node.named_children(&mut node.walk()) {
        if child.is_extra() || child.kind() == "keyword_array" {
            continue;
        }
        if !literals::string_literal(child, sql, "postgres") {
            return false;
        }
        count += 1;
    }
    count > 0
}
