//! Fixed builtin scalar results after live overload and identity resolution.
use super::super::projection::ProjectionSources;
use super::{common_postgres_type, infer, ExpressionType, UNTYPED_STRING};
use tree_sitter::Node;

pub(super) fn infer_builtin(
    name: &str,
    args: &[Node<'_>],
    sql: &str,
    sources: &ProjectionSources<'_>,
    dialect: &str,
    depth: usize,
) -> Option<ExpressionType> {
    if dialect != "postgres" {
        return None;
    }
    let values = || {
        args.iter()
            .map(|node| infer(*node, sql, sources, Some(dialect), depth + 1))
    };
    let fixed = match (name, args.len()) {
        ("lower" | "upper", 1) => Some("text"),
        ("length" | "char_length" | "character_length" | "octet_length", 1) => Some("integer"),
        ("replace", 3) => Some("text"),
        ("strpos", 2) => Some("integer"),
        _ => None,
    };
    if let Some(result) = fixed {
        let nullable = values().any(|value| value.is_none_or(|value| value.nullable));
        return Some(ExpressionType::new(result, nullable));
    }
    if name == "nullif" && args.len() == 2 {
        let values = values().collect::<Option<Vec<_>>>()?;
        let mut result = common_postgres_type(&values)?;
        result.nullable = true;
        return Some(result);
    }
    if matches!(name, "bool_and" | "bool_or" | "every") && args.len() == 1 {
        let argument = values().next()??;
        if matches!(argument.data_type.to_ascii_lowercase().as_str(), "null")
            || argument.data_type == UNTYPED_STRING
            || crate::postgres_types::builtin_type(&argument.data_type)
                .is_some_and(|ty| !ty.array && ty.name == "boolean")
        {
            return Some(ExpressionType::new("boolean", true));
        }
    }
    None
}
