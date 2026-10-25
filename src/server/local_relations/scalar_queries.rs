//! Infer each outermost scalar/row query in a projection once, using the enclosing
//! query budget and original source identity. Nested queries recurse normally.
use super::{expression_types::ExpressionType, query_output::Context};
use std::{collections::HashMap, ops::Range};
use tree_sitter::{Node, Tree};

const MAX_RETAINED_TYPE_BYTES: usize = 256 * 1024;

pub(super) fn infer(
    context: &Context<'_>,
    query: &str,
    upper: &str,
    tree: Option<&Tree>,
    projection: Range<usize>,
    depth: usize,
    budget: &mut usize,
) -> HashMap<(usize, usize), Vec<ExpressionType>> {
    let mut types = HashMap::new();
    let Some(tree) = tree else {
        return types;
    };
    if !query[projection.clone()].contains('(') {
        return types;
    }
    let mut queries = Vec::new();
    let mut visits = 32768;
    let mut remaining_type_bytes = MAX_RETAINED_TYPE_BYTES;
    collect(tree.root_node(), &projection, 0, &mut visits, &mut queries);
    for span in queries {
        if *budget == 0 {
            break;
        }
        let body = span.start + 1..span.end - 1;
        let inner = Context {
            schema: context.schema,
            uri: context.uri,
            line: context
                .line
                .saturating_add(query[..body.start].matches('\n').count() as u32),
            dialect: context.dialect,
            environment: context.environment,
        };
        let output = inner.query(&query[body.clone()], &upper[body], depth + 1, budget);
        if !output.known_arity
            || output.columns.is_empty()
            || output.columns.len() > super::expression_types::MAX_COMPOUND_ITEMS
        {
            continue;
        }
        let columns = inner.finish(output, &[]);
        let Some(bytes) = columns.iter().try_fold(0usize, |bytes, column| {
            bytes.checked_add(column.data_type.len())
        }) else {
            continue;
        };
        if bytes > remaining_type_bytes {
            continue;
        }
        remaining_type_bytes -= bytes;
        // A scalar query can produce no row. Do not inherit NOT NULL from the
        // selected column or assume cardinality from syntax alone.
        types.insert(
            (span.start, span.end),
            columns
                .into_iter()
                .map(|column| ExpressionType {
                    data_type: column.data_type,
                    nullable: true,
                })
                .collect(),
        );
    }
    types
}

fn collect(
    node: Node<'_>,
    projection: &Range<usize>,
    depth: usize,
    visits: &mut usize,
    queries: &mut Vec<Range<usize>>,
) {
    if *visits == 0
        || depth >= 64
        || queries.len() >= super::query_output::MAX_SET_BRANCHES
        || node.end_byte() <= projection.start
        || node.start_byte() >= projection.end
    {
        return;
    }
    *visits -= 1;
    // EXISTS accepts arbitrary output arity and is a predicate, not a scalar
    // value from its selected column. Its body needs no output-type inference.
    if node.kind() == "exists" || super::expression_types::quantified_operand(node).is_some() {
        return;
    }
    if node.kind() == "binary_expression"
        && node
            .child_by_field_name("operator")
            .is_some_and(|operator| matches!(operator.kind(), "keyword_in" | "not_in"))
    {
        if let Some(left) = node.child_by_field_name("left") {
            collect(left, projection, depth + 1, visits, queries);
        }
        return;
    }
    if node.kind() == "subquery" {
        if !node.has_error()
            && !node.is_missing()
            && node.start_byte() >= projection.start
            && node.end_byte() <= projection.end
        {
            queries.push(node.start_byte()..node.end_byte());
        }
        return;
    }
    for child in node.named_children(&mut node.walk()) {
        collect(child, projection, depth + 1, visits, queries);
        if *visits == 0 || queries.len() >= super::query_output::MAX_SET_BRANCHES {
            break;
        }
    }
}
