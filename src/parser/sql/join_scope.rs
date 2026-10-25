//! An ON predicate sees the operands of its own join, including the accumulated
//! left join chain, rather than every source in its SELECT block.
use super::types::{RelationAlias, RelationScope, SqlParser};
use std::ops::Range;
use tower_lsp::lsp_types::Position;
use tree_sitter::{Node, Tree};

fn join_operand_window(
    tree: &Tree,
    cursor: usize,
    start: usize,
    sqlite: bool,
) -> Option<(Node<'_>, Range<usize>)> {
    let at = cursor.min(tree.root_node().end_byte().saturating_sub(1));
    let mut node = tree.root_node().descendant_for_byte_range(at, at)?;
    loop {
        if node.kind() == "join" && node.start_byte() >= start {
            if let Some(predicate) = node.child_by_field_name("predicate") {
                let on = node
                    .named_children(&mut node.walk())
                    .find(|n| n.kind() == "keyword_on")?;
                if on.end_byte() <= cursor && cursor <= predicate.end_byte() {
                    let mut parent = node.parent()?;
                    while sqlite && parent.kind() == "join" {
                        parent = parent.parent()?;
                    }
                    let outer = node.named_children(&mut node.walk()).any(|n| {
                        matches!(n.kind(), "keyword_left" | "keyword_right" | "keyword_full")
                    });
                    let left = match parent.kind() {
                        // Comma binds less tightly than JOIN. Only the last
                        // direct base relation begins this joined item.
                        "from" => {
                            let mut cursor = parent.walk();
                            let mut relations = parent.named_children(&mut cursor).filter(|n| {
                                n.kind() == "relation" && n.start_byte() < node.start_byte()
                            });
                            if sqlite {
                                relations.next()?
                            } else {
                                relations.last()?
                            }
                        }
                        // The grammar can represent an explicit right chain
                        // as a join nested in the previous right operand.
                        "join" => parent
                            .named_children(&mut parent.walk())
                            .find(|n| n.kind() == "relation")?,
                        _ => return None,
                    };
                    // SQLite evaluates comma sources left-to-right. Its inner
                    // ON terms can refer to later inputs; an outer ON cannot.
                    let end = if sqlite && !outer {
                        parent.end_byte()
                    } else {
                        on.start_byte()
                    };
                    return Some((parent, left.start_byte()..end));
                }
            }
        }
        node = node.parent()?;
    }
}

fn join_scope_row(node: Node<'_>, source: &str, scope: &mut RelationScope) {
    let alias = node.child_by_field_name("alias");
    let value = node
        .named_children(&mut node.walk())
        .find(|n| matches!(n.kind(), "object_reference" | "subquery" | "invocation"));
    let Some(value) = value else {
        return;
    };
    let reference = match value.kind() {
        "subquery" => {
            let Some(alias) = alias else {
                return;
            };
            SqlParser::identifier_path_from_parts(&[SqlParser::identifier_last_part(
                &source[alias.byte_range()],
            )])
        }
        "invocation" => {
            let Some(target) = value
                .named_children(&mut value.walk())
                .find(|n| n.kind() == "object_reference")
            else {
                return;
            };
            source[target.byte_range()].to_owned()
        }
        _ => source[value.byte_range()].to_owned(),
    };
    if !scope
        .relations
        .iter()
        .any(|r| r.eq_ignore_ascii_case(&reference))
    {
        scope.relations.push(reference.clone());
    }
    let alias = alias.map(|alias| RelationAlias {
        name: SqlParser::identifier_last_part(&source[alias.byte_range()]),
        sql: source[alias.byte_range()].into(),
        relation: reference.clone(),
    });
    if let Some(alias) = &alias {
        scope.aliases.push(alias.clone());
    }
    scope
        .occurrences
        .as_mut()
        .expect("AST scope owns occurrences")
        .push((reference, alias));
}

impl SqlParser {
    /// Import only an explicitly requested outer member, after resolving each
    /// correlation edge and local alias shadow. Bare alias suggestions retain
    /// the immediate query's inputs.
    pub(crate) fn relation_scope_for_member(
        &self,
        tree: &Tree,
        source: &str,
        position: Position,
    ) -> Option<RelationScope> {
        let scopes = self.relation_scopes_at_position(tree, source, position);
        let mut result = scopes.first()?.clone();
        let Some(qualifier) = self.column_qualifier_at_position(source, position) else {
            return Some(result);
        };
        for (depth, scope) in scopes.iter().enumerate() {
            let alias = Self::resolve_relation_alias(&scope.aliases, &qualifier);
            let matches_reference = |reference: &str| {
                reference.eq_ignore_ascii_case(&qualifier)
                    || Self::identifier_qualifier(&qualifier).is_none()
                        && Self::identifier_last_part(reference).eq_ignore_ascii_case(&qualifier)
            };
            let unaliased = if let Some(rows) = &scope.occurrences {
                rows.iter()
                    .find(|(r, a)| a.is_none() && matches_reference(r))
                    .map(|(r, _)| r)
            } else {
                scope.relations.iter().find(|r| {
                    matches_reference(r)
                        && !scope
                            .aliases
                            .iter()
                            .any(|a| a.relation.eq_ignore_ascii_case(r))
                })
            };
            if let Some((reference, alias)) = alias
                .map(|a| (&a.relation, Some(a)))
                .or_else(|| unaliased.map(|r| (r, None)))
            {
                if depth > 0 {
                    if !result
                        .relations
                        .iter()
                        .any(|r| r.eq_ignore_ascii_case(reference))
                    {
                        result.relations.push(reference.clone());
                    }
                    if let Some(alias) = alias {
                        result.aliases.push(alias.clone());
                    }
                    if let Some(rows) = &mut result.occurrences {
                        rows.push((reference.clone(), alias.cloned()));
                    }
                }
                break;
            }
        }
        Some(result)
    }

    pub(crate) fn with_sqlite_joins(mut self) -> Self {
        self.sqlite_join_scopes = true;
        self
    }
    /// Byte ranges shared by diagnostics and row-binding consumers. A nested
    /// query passes its own start, so an ancestor ON is applied only when
    /// following the edge back to that ancestor query.
    pub(crate) fn join_operand_range(
        tree: &Tree,
        cursor: usize,
        start: usize,
    ) -> Option<Range<usize>> {
        join_operand_window(tree, cursor, start, false).map(|(_, range)| range)
    }

    pub(crate) fn join_operand_scope(
        &self,
        tree: &Tree,
        source: &str,
        position: Position,
        start: usize,
    ) -> Option<RelationScope> {
        let at = Self::byte_offset_for_position(source, position);
        if let Some(scope) = self.joined_source_scope(tree, source, position, start) {
            return Some(scope);
        }
        let (root, range) = join_operand_window(tree, at, start, self.sqlite_join_scopes)?;
        let mut scope = RelationScope {
            joined: None,
            start,
            relations: Vec::new(),
            aliases: Vec::new(),
            occurrences: Some(Vec::new()),
        };
        let mut walk = root.walk();
        loop {
            let node = walk.node();
            if node.start_byte() >= range.end {
                return Some(scope);
            }
            let row = matches!(
                node.kind(),
                "relation" | "lateral_join" | "lateral_cross_join"
            );
            if row && range.contains(&node.start_byte()) {
                join_scope_row(node, source, &mut scope);
            }
            if !row && node.kind() != "subquery" && !node.is_extra() && walk.goto_first_child() {
                continue;
            }
            loop {
                if walk.goto_next_sibling() {
                    break;
                }
                if !walk.goto_parent() {
                    return Some(scope);
                }
            }
        }
    }
}
