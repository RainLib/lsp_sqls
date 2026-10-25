//! Concrete live scalar returns use the same identity and argument binding as
//! signatures. Unresolved overloads never borrow the first candidate's type.
use super::super::projection::ProjectionSources;
use super::{infer, ExpressionType};
use crate::parser::SqlParser;
use crate::server::{
    routine_arguments::RoutineArguments,
    routine_identity::{routine_reference_before, RoutineIdentity},
    routine_overloads::canonical_type,
    routine_parameters::{routine_kind_matches, RoutineParameters},
};
use tree_sitter::Node;

pub(super) fn reference(node: Node<'_>, sql: &str, dialect: &str) -> Option<RoutineIdentity> {
    let reference = node
        .named_children(&mut node.walk())
        .find(|child| child.kind() == "object_reference")?;
    let text = reference.utf8_text(sql.as_bytes()).ok()?;
    if text.len() > 64 * 1024 {
        return None;
    }
    let content = SqlParser::mask_sql_operand_context_for_dialect(text, dialect);
    let (name, _) = routine_reference_before(text, &content, text.len(), dialect)?;
    RoutineIdentity::parse(&name, dialect)
}

fn concrete_type(ty: &str) -> Option<&str> {
    let ty = ty.trim();
    if ty.is_empty() || ty.len() > 256 || ty.contains(['\0', '\r', '\n']) {
        return None;
    }
    let lower = ty.to_ascii_lowercase();
    let base = lower
        .strip_prefix("pg_catalog.")
        .unwrap_or(&lower)
        .trim_end_matches("[]")
        .trim();
    if base.starts_with("setof ")
        || base.starts_with("table ")
        || base.starts_with("table(")
        || matches!(
            base,
            "void"
                | "record"
                | "unknown"
                | "trigger"
                | "event_trigger"
                | "internal"
                | "cstring"
                | "opaque"
                | "any"
                | "anyelement"
                | "anyarray"
                | "anynonarray"
                | "anyenum"
                | "anyrange"
                | "anymultirange"
                | "anycompatible"
                | "anycompatiblearray"
                | "anycompatiblenonarray"
                | "anycompatiblerange"
                | "anycompatiblemultirange"
        )
    {
        return None;
    }
    Some(ty)
}
fn same_type(left: &str, right: &str, dialect: &str) -> bool {
    left == right
        || canonical_type(left, dialect)
            .zip(canonical_type(right, dialect))
            .is_some_and(|(left, right)| left == right)
}

// Some(None) deliberately blocks built-in fallback when matching live metadata
// exists but its declaration, binding or return identity is unresolved.
pub(super) fn resolve(
    node: Node<'_>,
    sql: &str,
    sources: &ProjectionSources<'_>,
    identity: &RoutineIdentity,
    dialect: &str,
    depth: usize,
) -> Option<Option<ExpressionType>> {
    let schema = sources.routine_metadata(identity, dialect)?;
    let overloads = schema
        .functions
        .iter()
        .filter(|function| {
            identity.matches(function, dialect) && routine_kind_matches(function, false)
        })
        .collect::<Vec<_>>();
    if overloads.is_empty() {
        return None;
    }
    if node.end_byte() - node.start_byte() > 64 * 1024 {
        return Some(None);
    }
    let nodes = node
        .children_by_field_name("parameter", &mut node.walk())
        .collect::<Vec<_>>();
    if nodes.len() > super::MAX_COMPOUND_ITEMS {
        return Some(None);
    }
    let ranges = nodes
        .iter()
        .map(|node| node.byte_range())
        .collect::<Vec<_>>();
    let mut arguments = RoutineArguments::from_ranges(sql, &ranges, true, dialect);
    for marker in node
        .named_children(&mut node.walk())
        .filter(|child| child.kind() == "keyword_variadic")
    {
        let Some(index) = nodes
            .iter()
            .position(|argument| argument.start_byte() >= marker.end_byte())
        else {
            return Some(None);
        };
        arguments.values[index].explicit = true;
    }
    let types = nodes
        .iter()
        .map(|node| {
            let value = if node.kind() == "named_argument" {
                node.child_by_field_name("value")
            } else {
                Some(*node)
            };
            value
                .and_then(|value| infer(value, sql, sources, Some(dialect), depth + 1))
                .and_then(|ty| canonical_type(&ty.data_type, dialect))
        })
        .collect::<Vec<_>>();
    let mut candidates = Vec::new();
    for function in overloads {
        let parameters = RoutineParameters::new(function, dialect, false);
        let binding = arguments.bind(&parameters, dialect);
        if !binding.fits {
            continue;
        }
        let exact = arguments
            .values
            .iter()
            .enumerate()
            .all(|(index, argument)| {
                types[index].as_ref().is_some_and(|actual| {
                    binding.slots[index]
                        .and_then(|slot| parameters.data_type(slot, argument.explicit))
                        .and_then(|expected| canonical_type(expected, dialect))
                        .as_ref()
                        == Some(actual)
                })
            });
        candidates.push((function, exact, parameters.variadic_index().is_none()));
    }
    let exact = candidates
        .iter()
        .filter(|(_, exact, _)| *exact)
        .collect::<Vec<_>>();
    let has_exact = !exact.is_empty();
    let chosen = if exact.is_empty() {
        candidates.iter().collect::<Vec<_>>()
    } else {
        exact
    };
    let prefer_fixed = has_exact
        && crate::sql_lexical::lexical_rules_for(dialect).placeholder
            == crate::placeholder::SqlPlaceholderDialect::Postgres
        && !arguments.values.iter().any(|argument| argument.explicit)
        && chosen.iter().any(|(_, _, fixed)| *fixed);
    let mut result = None;
    for (function, _, fixed) in chosen {
        if prefer_fixed && !fixed {
            continue;
        }
        let Some(ty) = concrete_type(&function.return_type) else {
            return Some(None);
        };
        if result.is_some_and(|previous| !same_type(previous, ty, dialect)) {
            return Some(None);
        }
        if result.is_none() {
            result = Some(ty);
        }
    }
    Some(result.map(|ty| ExpressionType::new(ty, true)))
}
