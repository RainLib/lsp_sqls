//! Field roles and query blocks feed the shared row-binding resolver.
use super::InspectionContext;
use crate::dialects::common::ColumnScopes;
use crate::server::local_relations::augment_schema_with_local_relations_and_routines;
use crate::{
    parser::{RelationAlias, RelationScope, SqlParser},
    schema::Schema,
};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    ops::Range,
};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use tree_sitter::{Node, Tree};
#[cfg(test)]
thread_local! {static WALK_VISITS:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};}
#[cfg(test)]
#[path = "ambiguity_tests.rs"]
mod tests;
struct Row {
    at: usize,
    binding: RelationScope,
    using: Vec<String>,
    natural: bool,
    local: bool,
}
struct Field {
    range: Range<usize>,
    name: String,
    fold: bool,
    output_order: bool,
}
struct Cte {
    name: String,
    owner: Range<usize>,
    end: usize,
    recursive: bool,
}
impl Cte {
    fn visible(&self, at: usize) -> bool {
        self.owner.contains(&at) && (self.end <= at || self.recursive)
    }
}
struct Query<'t> {
    range: Range<usize>,
    node: Node<'t>,
    parent: Option<usize>,
    isolated: bool,
    left_limit: Option<usize>,
    rows: Vec<Row>,
    fields: Vec<Field>,
    outputs: HashMap<String, usize>,
    star: bool,
}
fn child<'t>(n: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut c = n.walk();
    let found = n.named_children(&mut c).find(|n| n.kind() == kind);
    found
}
fn walk<'t>(tree: &'t Tree, mut visit: impl FnMut(Node<'t>)) {
    let mut c = tree.walk();
    loop {
        let n = c.node();
        #[cfg(test)]
        WALK_VISITS.with(|count| count.set(count.get() + 1));
        visit(n);
        if (!n.is_extra() || n.is_error()) && c.goto_first_child() {
            continue;
        }
        loop {
            if c.goto_next_sibling() {
                break;
            }
            if !c.goto_parent() {
                return;
            }
        }
    }
}
fn query<'t>(n: Node<'t>, mask: &str) -> Option<Query<'t>> {
    if !matches!(n.kind(), "select" | "update" | "delete") {
        return None;
    }
    let container = if n.kind() == "update" { n } else { n.parent()? };
    let mut end = container.end_byte();
    // Recovery can split a missing-ON tail into a sibling ERROR instead of
    // retaining it inside the statement. Extend only across adjacent error
    // nodes in the same lexical statement, never across a semicolon/SELECT.
    if container.kind() == "statement"
        && container.next_named_sibling().is_some_and(|s| s.is_error())
    {
        let boundary = SqlParser::next_statement_end(mask, n.start_byte());
        let mut next = container.next_named_sibling();
        while let Some(error) = next.filter(|s| s.is_error()) {
            if error.start_byte() >= boundary || !mask[end..error.start_byte()].trim().is_empty() {
                break;
            }
            end = error.end_byte().min(boundary);
            next = error.next_named_sibling();
        }
    }
    if n.kind() == "select" {
        let mut next = n.next_named_sibling();
        while let Some(s) = next {
            if matches!(
                s.kind(),
                "keyword_union"
                    | "keyword_except"
                    | "keyword_intersect"
                    | "select"
                    | "set_operation"
            ) {
                end = s.start_byte();
                break;
            }
            next = s.next_named_sibling();
        }
    }
    let mut ancestor = Some(container);
    let mut isolated = false;
    let mut left_limit = None;
    while let Some(p) = ancestor {
        if p.kind() == "cte" {
            isolated = true;
            break;
        }
        if p.kind() == "subquery" {
            let before = mask[..p.start_byte()].trim_end();
            let last = before
                .rsplit(|c: char| !SqlParser::is_identifier_char(c))
                .find(|s| !s.is_empty())
                .unwrap_or("");
            if matches!(last, "LATERAL" | "APPLY") {
                left_limit = Some(p.start_byte());
            } else if p.parent().is_some_and(|a| a.kind() == "relation") {
                isolated = true;
            }
            break;
        }
        if p.kind() == "statement" && p.parent().is_none_or(|a| a.kind() != "cte") {
            break;
        }
        ancestor = p.parent();
    }
    Some(Query {
        range: n.start_byte()..end,
        node: n,
        parent: None,
        isolated,
        left_limit,
        rows: Vec::new(),
        fields: Vec::new(),
        outputs: HashMap::new(),
        star: false,
    })
}
fn identifier(source: &str, n: Node<'_>) -> String {
    SqlParser::identifier_last_part(&source[n.byte_range()])
}
fn row(n: Node<'_>, text: &str) -> Option<Row> {
    let alias = n.child_by_field_name("alias");
    let value = child(n, "object_reference")
        .or_else(|| child(n, "subquery"))
        .or_else(|| child(n, "invocation"))?;
    let local = value.kind() != "object_reference";
    let reference = if value.kind() == "subquery" {
        SqlParser::identifier_path_from_parts(&[identifier(text, alias?)])
    } else if value.kind() == "invocation" {
        let target = child(value, "object_reference")?;
        text[target.byte_range()].to_owned()
    } else {
        text[value.byte_range()].to_owned()
    };
    let aliases = alias
        .map(|a| RelationAlias {
            name: identifier(text, a),
            sql: text[a.byte_range()].into(),
            relation: reference.clone(),
        })
        .into_iter()
        .collect();
    let mut using = Vec::new();
    let mut natural = false;
    if let Some(join) = n.parent().filter(|p| p.kind() == "join") {
        natural = child(join, "keyword_natural").is_some();
        if child(join, "keyword_using").is_some() {
            if let Some(list) = child(join, "list") {
                let mut c = list.walk();
                using.extend(
                    list.named_children(&mut c)
                        .filter_map(|n| {
                            if n.kind() == "identifier" {
                                Some(n)
                            } else if n.kind() == "column" {
                                n.named_child(0)
                            } else {
                                None
                            }
                        })
                        .map(|n| identifier(text, n)),
                );
            }
        }
    }
    Some(Row {
        at: n.start_byte(),
        binding: RelationScope {
            joined: None,
            occurrences: None,
            start: n.start_byte(),
            relations: vec![reference],
            aliases,
        },
        using,
        natural,
        local,
    })
}
fn field(n: Node<'_>, text: &str, dialect: &str) -> Option<Field> {
    let quoted = n.kind() == "literal"
        && text[n.byte_range()].starts_with('"')
        && text[n.byte_range()].ends_with('"')
        && !crate::sql_lexical::lexical_rules_for(dialect).double_quoted_strings;
    if n.kind() != "field" && !quoted || child(n, "object_reference").is_some() {
        return None;
    }
    if let Some(parent) = n.parent().filter(|p| p.kind() == "assignment") {
        if parent.child_by_field_name("left") == Some(n) {
            return None;
        }
    }
    let name = if quoted {
        n
    } else {
        n.child_by_field_name("name")?
    };
    let raw = &text[name.byte_range()];
    let fold = !matches!(raw.as_bytes().first(), Some(b'"' | b'`' | b'['));
    let output_order = n.parent().is_some_and(|p| p.kind() == "order_target");
    Some(Field {
        range: name.byte_range(),
        name: identifier(text, name),
        fold,
        output_order,
    })
}
fn graphs<'t>(ctx: &InspectionContext<'_>, tree: &'t Tree) -> (Vec<Query<'t>>, Vec<Cte>) {
    let mut q = Vec::<Query<'t>>::new();
    let mut ctes = Vec::new();
    let mut stack = Vec::<usize>::new();
    // Tree cursor preorder already visits query starts in source order. Bind
    // fields and rows as they are visited, popping scopes when their ranges
    // close (including a SELECT branch ending before UNION). This avoids a
    // second full walk and a binary owner search for every node.
    walk(tree, |n| {
        // Punctuation cannot introduce a query, row or field role. Closing a
        // scope can wait until the next named node needing an owner.
        if !n.is_named() && !n.is_error() {
            return;
        }
        if let Some(mut s) = query(n, &ctx.normalized) {
            while stack
                .last()
                .is_some_and(|p| q[*p].range.end < s.range.end || q[*p].range.end <= s.range.start)
            {
                stack.pop();
            }
            s.parent = stack.last().copied();
            stack.push(q.len());
            q.push(s);
        } else {
            while stack
                .last()
                .is_some_and(|p| q[*p].range.end <= n.start_byte())
            {
                stack.pop();
            }
        }
        if n.kind() == "cte" {
            if let Some(name) = n.named_child(0) {
                if let Some(owner) = n.parent() {
                    ctes.push(Cte {
                        name: identifier(ctx.text, name).to_ascii_lowercase(),
                        owner: owner.byte_range(),
                        end: n.end_byte(),
                        recursive: child(owner, "keyword_recursive").is_some()
                            || ctx.dialect == "sqlserver",
                    });
                }
            }
        }
        let Some(i) = stack.last().copied() else {
            return;
        };
        if n.kind() == "relation" {
            if let Some(r) = row(n, ctx.text) {
                q[i].rows.push(r);
            }
        }
        if n.kind() == "object_reference" && n.parent().is_some_and(|p| p.kind() == "from") {
            q[i].rows.push(Row {
                at: n.start_byte(),
                binding: RelationScope {
                    joined: None,
                    occurrences: None,
                    start: n.start_byte(),
                    relations: vec![ctx.text[n.byte_range()].into()],
                    aliases: Vec::new(),
                },
                using: Vec::new(),
                natural: false,
                local: false,
            });
        }
        if let Some(f) = field(n, ctx.text, ctx.dialect) {
            q[i].fields.push(f);
        }
        if n.kind() == "all_fields" {
            q[i].star = true;
        }
        if n.kind() == "term" {
            let out = n.child_by_field_name("alias").or_else(|| {
                n.child_by_field_name("value")
                    .filter(|v| v.kind() == "field")
                    .and_then(|v| v.child_by_field_name("name"))
            });
            if let Some(out) = out {
                *q[i]
                    .outputs
                    .entry(identifier(ctx.text, out).to_ascii_lowercase())
                    .or_default() += 1;
            }
        }
    });
    // A missing ON can leave the right relation in a recovery ERROR node.
    // Reuse existing scope extraction once for that query, without parsing.
    let parser = SqlParser::new();
    for query in &mut q {
        if !query
            .node
            .parent()
            .is_some_and(|n| n.has_error() || query.range.end > n.end_byte())
        {
            continue;
        }
        let text = &ctx.text[query.range.clone()];
        let at = query
            .fields
            .first()
            .map_or(query.node.end_byte(), |f| f.range.end)
            .saturating_sub(query.range.start)
            .min(text.len());
        let pos = crate::position::byte_position_at_end(&text[..at]);
        let references = parser.extract_row_sources_at_position(tree, text, pos);
        let aliases = SqlParser::relation_aliases_at_position(text, pos);
        let mut recovered = Vec::new();
        for reference in references {
            let bound: Vec<_> = aliases
                .iter()
                .filter(|a| a.relation.eq_ignore_ascii_case(&reference))
                .cloned()
                .collect();
            if bound.is_empty() {
                recovered.push(Row {
                    at: query.range.start,
                    binding: RelationScope {
                        joined: None,
                        occurrences: None,
                        start: query.range.start,
                        relations: vec![reference],
                        aliases: Vec::new(),
                    },
                    using: Vec::new(),
                    natural: false,
                    local: false,
                });
            } else {
                for alias in bound {
                    recovered.push(Row {
                        at: query.range.start,
                        binding: RelationScope {
                            joined: None,
                            occurrences: None,
                            start: query.range.start,
                            relations: vec![reference.clone()],
                            aliases: vec![alias],
                        },
                        using: Vec::new(),
                        natural: false,
                        local: false,
                    });
                }
            }
        }
        if recovered.len() > query.rows.len() {
            query.rows = recovered;
        }
    }
    (q, ctes)
}
fn merged(rows: &[&Row], names: &[Option<Vec<String>>], fold: bool) -> HashMap<String, usize> {
    let mut counts = HashMap::<String, usize>::new();
    let mut reductions = HashMap::<String, usize>::new();
    for (i, cols) in names.iter().enumerate() {
        let Some(cols) = cols else {
            continue;
        };
        let set: HashSet<String> = cols
            .iter()
            .map(|c| {
                if fold {
                    c.to_ascii_lowercase()
                } else {
                    c.clone()
                }
            })
            .collect();
        for name in set {
            let before = counts.get(&name).copied().unwrap_or(0);
            let coalesce = rows.get(i).is_some_and(|r| {
                r.natural
                    || r.using.iter().any(|s| {
                        if fold {
                            s.eq_ignore_ascii_case(&name)
                        } else {
                            s == &name
                        }
                    })
            });
            if before == 1 && coalesce {
                *reductions.entry(name).or_default() += 1;
            } else {
                *counts.entry(name).or_default() += 1;
            }
        }
    }
    reductions
}
#[cfg(test)]
pub(super) fn diagnostics(ctx: &InspectionContext<'_>, schema: Option<&Schema>) -> Vec<Diagnostic> {
    diagnostics_with_metadata(ctx, schema, None)
}
pub(super) fn diagnostics_with_metadata(
    ctx: &InspectionContext<'_>,
    schema: Option<&Schema>,
    manager: Option<&crate::schema::SchemaManager>,
) -> Vec<Diagnostic> {
    let Some(schema) = schema.filter(|s| manager.is_some() || !s.tables.is_empty()) else {
        return Vec::new();
    };
    let Some(tree) = ctx.syntax_tree() else {
        return Vec::new();
    };
    let (queries, ctes) = graphs(ctx, tree);
    let parser = SqlParser::new_with_placeholder_dialect(
        crate::sql_lexical::lexical_rules_for(ctx.dialect).placeholder,
    );
    let mut out = Vec::new();
    let spans: Vec<_> = ctx.statements().collect();
    for (index, q) in queries.iter().enumerate() {
        if q.fields.is_empty() {
            continue;
        }
        let mut levels = Vec::<Vec<&Row>>::new();
        let mut current = Some(index);
        let mut limit = None;
        let mut at = q.range.start;
        while let Some(i) = current {
            let scope = &queries[i];
            let operands = SqlParser::join_operand_range(tree, at, scope.range.start);
            levels.push(
                scope
                    .rows
                    .iter()
                    .filter(|r| {
                        limit.is_none_or(|at| r.at < at)
                            && operands.as_ref().is_none_or(|range| range.contains(&r.at))
                    })
                    .collect(),
            );
            if scope.isolated {
                break;
            }
            limit = scope.left_limit;
            at = scope.range.start;
            current = scope.parent;
        }
        if !levels.iter().any(|r| r.len() >= 2) {
            continue;
        }
        let cursor = q.fields[0].range.end;
        let joined = parser.has_joined_sources(
            tree,
            ctx.text,
            crate::position::byte_position_at_end(&ctx.text[..cursor]),
        );
        let local = levels.iter().flatten().any(|r| {
            r.local
                || r.binding.relations.iter().any(|name| {
                    ctes.iter().any(|c| {
                        c.visible(cursor)
                            && c.name == SqlParser::identifier_last_part(name).to_ascii_lowercase()
                    }) || !schema
                        .tables
                        .iter()
                        .any(|t| crate::relation_bindings::table_matches(schema, t, name))
                })
        });
        let augmented = if local {
            Cow::Owned(augment_schema_with_local_relations_and_routines(
                schema.clone(),
                ctx.text,
                ctx.range(cursor, cursor).start,
                "oxide://inspection/query.sql",
                Some(ctx.dialect),
                manager,
            ))
        } else {
            Cow::Borrowed(schema)
        };
        // Reuse the augmented schema for all fields. Only the immediate row
        // group changes between SELECT/WHERE and each separate ON predicate.
        let mut fields_by_operands = HashMap::<Option<(usize, usize)>, Vec<&Field>>::new();
        for field in &q.fields {
            let operands = if joined {
                let mut node = tree
                    .root_node()
                    .descendant_for_byte_range(field.range.start, field.range.start);
                let mut key = None;
                while let Some(n) = node.filter(|n| n.start_byte() >= q.range.start) {
                    if n.kind() == "join"
                        && n.child_by_field_name("predicate")
                            .is_some_and(|p| p.byte_range().contains(&field.range.start))
                    {
                        key = Some((n.start_byte(), n.end_byte()));
                        break;
                    }
                    node = n.parent();
                }
                key
            } else {
                SqlParser::join_operand_range(tree, field.range.start, q.range.start)
                    .map(|range| (range.start, range.end))
            };
            fields_by_operands.entry(operands).or_default().push(field);
        }
        for (operands, fields) in fields_by_operands {
            let levels: Vec<Vec<&Row>> = levels
                .iter()
                .enumerate()
                .map(|(i, rows)| {
                    rows.iter()
                        .copied()
                        .filter(|r| {
                            i != 0
                                || operands.is_none_or(|(start, end)| (start..end).contains(&r.at))
                        })
                        .collect()
                })
                .collect();
            let groups: Vec<_> = levels
                .iter()
                .map(|rows| rows.iter().map(|r| r.binding.clone()).collect())
                .collect();
            let mut unknown = HashSet::new();
            for (i, rows) in levels.iter().enumerate() {
                for (j, r) in rows.iter().enumerate() {
                    let expected = r.local
                        || r.binding.relations.iter().any(|name| {
                            SqlParser::identifier_qualifier(name).is_none()
                                && ctes.iter().any(|c| {
                                    c.visible(cursor)
                                        && c.name
                                            == SqlParser::identifier_last_part(name)
                                                .to_ascii_lowercase()
                                })
                        });
                    let reference = r
                        .binding
                        .aliases
                        .first()
                        .map(|alias| {
                            crate::dialects::common::row_reference(
                                &augmented,
                                &r.binding.aliases,
                                &alias.sql,
                            )
                        })
                        .or_else(|| r.binding.relations.first().map(String::as_str));
                    // A CTE alias names a row occurrence; its schema overlay
                    // keeps the CTE's relation name. Use the same resolver as
                    // completion instead of looking for an overlay by alias.
                    let known_local = reference
                        .and_then(|name| {
                            crate::relation_bindings::projection_table(&augmented, manager, name)
                        })
                        .is_some_and(|table| crate::relation_bindings::query_local(&table));
                    if expected && !known_local {
                        unknown.insert((i, j));
                    }
                }
            }
            let bindings = if joined {
                ColumnScopes::new_with_metadata(
                    &parser,
                    tree,
                    ctx.text,
                    crate::position::byte_position_at_end(&ctx.text[..fields[0].range.start]),
                    &augmented,
                    0,
                    manager,
                )
            } else {
                ColumnScopes::from_row_groups_with_metadata(&augmented, &groups, &unknown, manager)
            };
            let names = bindings.row_columns();
            let reductions_folded: Vec<_> = levels
                .iter()
                .zip(&names)
                .map(|(r, n)| merged(r, n, true))
                .collect();
            let reductions_exact: Vec<_> = levels
                .iter()
                .zip(&names)
                .map(|(r, n)| merged(r, n, false))
                .collect();
            for f in fields {
                if f.output_order
                    && !q.star
                    && q.outputs.get(&f.name.to_ascii_lowercase()) == Some(&1)
                {
                    continue;
                }
                if !bindings.ambiguous(
                    &f.name,
                    f.fold,
                    if f.fold {
                        &reductions_folded
                    } else {
                        &reductions_exact
                    },
                ) {
                    continue;
                }
                let at = spans
                    .partition_point(|s| s.start <= f.range.start)
                    .saturating_sub(1);
                if let Some(span) = spans.get(at) {
                    if ctx.suppressions_at(span, f.range.start) & 8 != 0 {
                        continue;
                    }
                }
                out.push(Diagnostic{range:ctx.range(f.range.start,f.range.end),severity:Some(DiagnosticSeverity::WARNING),code:Some(NumberOrString::String("OXIDE004".into())),source:Some("oxide-inspections".into()),message:format!("Column `{}` exists in multiple referenced relations; qualify it with an alias.",&ctx.text[f.range.clone()]),..Default::default()});
            }
        }
    }
    out.sort_by_key(|d| (d.range.start.line, d.range.start.character));
    out
}
