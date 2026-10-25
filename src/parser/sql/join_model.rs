//! Parenthesized sources, comma precedence and implicit join keys retain a
//! request-owned layout. Existing plain-ON recovery remains a separate fallback.
use super::types::{JoinExpr, JoinKind, RelationAlias, RelationScope, SqlParser};
use tower_lsp::lsp_types::Position;
use tree_sitter::{Node, Tree};

fn joined_child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    node.named_children(&mut node.walk())
        .find(|n| n.kind() == kind)
}
fn joined_alias(node: Node<'_>, source: &str, reference: &str) -> Option<RelationAlias> {
    let alias = node.child_by_field_name("alias")?;
    Some(RelationAlias {
        name: SqlParser::identifier_last_part(&source[alias.byte_range()]),
        sql: source[alias.byte_range()].into(),
        relation: reference.into(),
    })
}
fn joined_columns(node: Node<'_>, source: &str) -> Vec<String> {
    joined_child(node, "list")
        .map(|list| {
            list.named_children(&mut list.walk())
                .filter_map(|n| match n.kind() {
                    "identifier" => Some(n),
                    "column" => n.named_child(0),
                    _ => None,
                })
                .map(|n| SqlParser::identifier_last_part(&source[n.byte_range()]))
                .collect()
        })
        .unwrap_or_default()
}
fn joined_feature(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    loop {
        let n = cursor.node();
        if matches!(n.kind(), "parenthesized_join" | "lateral_view")
            || n.kind() == "join"
                && (joined_child(n, "keyword_natural").is_some()
                    || joined_child(n, "keyword_using").is_some())
        {
            return true;
        }
        if matches!(
            n.kind(),
            "from" | "relation" | "join" | "cross_join" | "lateral_join" | "lateral_cross_join"
        ) && cursor.goto_first_child()
        {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return false;
            }
        }
    }
}
fn joined_from(tree: &Tree, cursor: usize, start: usize) -> Option<Node<'_>> {
    let at = cursor.min(tree.root_node().end_byte().saturating_sub(1));
    let mut node = tree.root_node().descendant_for_byte_range(at, at)?;
    loop {
        if node.kind() == "from" && node.start_byte() >= start {
            return Some(node);
        }
        if matches!(
            node.kind(),
            "statement" | "set_operation" | "update" | "delete"
        ) {
            let branch = node
                .named_children(&mut node.walk())
                .filter(|n| n.kind() == "select" && n.start_byte() <= at)
                .last()
                .map_or(start, |n| n.start_byte().max(start));
            let end = node
                .named_children(&mut node.walk())
                .find(|n| n.kind() == "select" && n.start_byte() > at)
                .map_or(node.end_byte(), |n| n.start_byte());
            if let Some(from) = node
                .named_children(&mut node.walk())
                .find(|n| n.kind() == "from" && branch <= n.start_byte() && n.start_byte() < end)
            {
                return Some(from);
            }
        }
        node = node.parent()?;
    }
}
fn joined_source(
    node: Node<'_>,
    source: &str,
    depth: usize,
    remaining: &mut usize,
    sqlite: bool,
) -> Option<JoinExpr> {
    if depth >= 32 || *remaining == 0 {
        return None;
    }
    *remaining -= 1;
    let value = node.named_children(&mut node.walk()).find(|n| {
        matches!(
            n.kind(),
            "object_reference" | "subquery" | "invocation" | "parenthesized_join"
        )
    })?;
    if value.kind() == "parenthesized_join" {
        let input = joined_body(value, source, depth + 1, remaining, sqlite)?;
        let alias = joined_alias(node, source, "").map(|mut a| {
            a.relation = SqlParser::identifier_path_from_parts(std::slice::from_ref(&a.name));
            a
        });
        return joined_views(
            node,
            source,
            JoinExpr::Group {
                input: Box::new(input),
                alias,
                columns: joined_columns(node, source),
                range: node.byte_range(),
            },
            remaining,
        );
    }
    let reference = match value.kind() {
        "subquery" => node
            .child_by_field_name("alias")
            .map(|alias| {
                SqlParser::identifier_path_from_parts(&[SqlParser::identifier_last_part(
                    &source[alias.byte_range()],
                )])
            })
            .unwrap_or_default(),
        "invocation" => source[joined_child(value, "object_reference")?.byte_range()].to_owned(),
        _ => source[value.byte_range()].to_owned(),
    };
    let mut local = value.kind() != "object_reference";
    if !local && SqlParser::identifier_qualifier(&reference).is_none() {
        let mut ancestor = node.parent();
        while let Some(owner) = ancestor {
            if owner.kind() == "statement" {
                let recursive = joined_child(owner, "keyword_recursive").is_some();
                local |= owner
                    .named_children(&mut owner.walk())
                    .filter(|n| {
                        n.kind() == "cte" && (n.end_byte() <= node.start_byte() || recursive)
                    })
                    .any(|cte| {
                        cte.named_child(0).is_some_and(|n| {
                            SqlParser::identifier_last_part(&source[n.byte_range()])
                                .eq_ignore_ascii_case(&SqlParser::identifier_last_part(&reference))
                        })
                    });
            }
            ancestor = owner.parent();
        }
    }
    joined_views(
        node,
        source,
        JoinExpr::Relation {
            alias: joined_alias(node, source, &reference),
            reference,
            range: node.start_byte()
                ..joined_child(node, "lateral_view").map_or(node.end_byte(), |n| n.start_byte()),
            local,
        },
        remaining,
    )
}
fn joined_views(
    node: Node<'_>,
    source: &str,
    mut input: JoinExpr,
    remaining: &mut usize,
) -> Option<JoinExpr> {
    for view in node
        .named_children(&mut node.walk())
        .filter(|n| n.kind() == "lateral_view")
    {
        if *remaining == 0 {
            return None;
        }
        *remaining -= 1;
        let alias = joined_alias(view, source, "")?;
        let reference = SqlParser::identifier_path_from_parts(std::slice::from_ref(&alias.name));
        let alias = RelationAlias {
            relation: reference.clone(),
            ..alias
        };
        input = joined_cross(
            input,
            JoinExpr::Relation {
                reference,
                alias: Some(alias),
                local: true,
                range: view.byte_range(),
            },
        );
    }
    Some(input)
}
fn joined_tail(
    left: JoinExpr,
    node: Node<'_>,
    source: &str,
    depth: usize,
    remaining: &mut usize,
    sqlite: bool,
) -> Option<JoinExpr> {
    if depth >= 32 {
        return None;
    }
    let mut right = joined_source(
        if node.kind() == "lateral_join" || node.kind() == "lateral_cross_join" {
            node
        } else {
            joined_child(node, "relation")?
        },
        source,
        depth + 1,
        remaining,
        sqlite,
    )?;
    if let Some(nested) = joined_child(node, "join") {
        right = joined_tail(right, nested, source, depth + 1, remaining, sqlite)?;
    }
    let outer_apply = joined_child(node, "keyword_cross")
        .is_some_and(|n| source[n.byte_range()].eq_ignore_ascii_case("OUTER"))
        && joined_child(node, "keyword_join")
            .is_some_and(|n| SqlParser::apply_keyword_range(n, source).is_some());
    let kind = if outer_apply || joined_child(node, "keyword_left").is_some() {
        JoinKind::Left
    } else if joined_child(node, "keyword_right").is_some() {
        JoinKind::Right
    } else if joined_child(node, "keyword_full").is_some() {
        JoinKind::Full
    } else if node.kind().contains("cross") {
        JoinKind::Cross
    } else {
        JoinKind::Inner
    };
    let predicate = node
        .child_by_field_name("predicate")
        .or_else(|| joined_child(node, "keyword_on").and_then(|on| on.next_named_sibling()))
        .and_then(|p| joined_child(node, "keyword_on").map(|on| on.end_byte()..p.end_byte()));
    Some(JoinExpr::Join {
        left: Box::new(left),
        right: Box::new(right),
        kind,
        keys: if joined_child(node, "keyword_using").is_some() {
            joined_columns(node, source)
        } else {
            Vec::new()
        },
        natural: joined_child(node, "keyword_natural").is_some(),
        predicate,
    })
}
fn joined_cross(left: JoinExpr, right: JoinExpr) -> JoinExpr {
    JoinExpr::Join {
        left: Box::new(left),
        right: Box::new(right),
        kind: JoinKind::Cross,
        keys: Vec::new(),
        natural: false,
        predicate: None,
    }
}
fn joined_body(
    node: Node<'_>,
    source: &str,
    depth: usize,
    remaining: &mut usize,
    sqlite: bool,
) -> Option<JoinExpr> {
    let mut current = None;
    let mut completed = None;
    for child in node.named_children(&mut node.walk()) {
        if child.kind() == "relation" {
            let next = joined_source(child, source, depth + 1, remaining, sqlite)?;
            if let Some(left) = current.take() {
                if sqlite {
                    current = Some(joined_cross(left, next));
                    continue;
                }
                completed = Some(match completed.take() {
                    Some(prefix) => joined_cross(prefix, left),
                    None => left,
                });
            }
            current = Some(next);
        } else if matches!(
            child.kind(),
            "join" | "cross_join" | "lateral_join" | "lateral_cross_join"
        ) {
            current = Some(joined_tail(
                current.take()?,
                child,
                source,
                depth + 1,
                remaining,
                sqlite,
            )?);
        }
    }
    let current = current?;
    Some(match completed {
        Some(prefix) => joined_cross(prefix, current),
        None => current,
    })
}
fn joined_condition(expr: &JoinExpr, cursor: usize, sqlite: bool) -> Option<(JoinExpr, bool)> {
    match expr {
        JoinExpr::Group { input, alias, .. } => joined_condition(input, cursor, sqlite)
            .map(|(input, future)| (input, future && alias.is_none())),
        JoinExpr::Join {
            left,
            right,
            predicate,
            kind,
            ..
        } => {
            if let Some((input, future)) = joined_condition(left, cursor, sqlite) {
                let future = future && !matches!(kind, JoinKind::Right | JoinKind::Full);
                return Some((
                    if future {
                        joined_cross(input, (**right).clone())
                    } else {
                        input
                    },
                    future,
                ));
            }
            joined_condition(right, cursor, sqlite).or_else(|| {
                predicate
                    .as_ref()
                    .filter(|r| r.start <= cursor && cursor <= r.end)
                    .map(|_| {
                        (
                            joined_cross((**left).clone(), (**right).clone()),
                            sqlite && *kind == JoinKind::Inner,
                        )
                    })
            })
        }
        _ => None,
    }
}
fn joined_occurrences(expr: &JoinExpr, scope: &mut RelationScope) {
    match expr {
        JoinExpr::Join { left, right, .. } => {
            joined_occurrences(left, scope);
            joined_occurrences(right, scope);
        }
        JoinExpr::Group {
            input, alias: None, ..
        } => joined_occurrences(input, scope),
        JoinExpr::Group {
            alias: Some(alias), ..
        } => joined_occurrence(&alias.relation, Some(alias), scope),
        JoinExpr::Relation {
            reference, alias, ..
        } => joined_occurrence(reference, alias.as_ref(), scope),
        JoinExpr::Unknown | JoinExpr::Empty => {}
    }
}
fn joined_occurrence(reference: &str, alias: Option<&RelationAlias>, scope: &mut RelationScope) {
    if !scope
        .relations
        .iter()
        .any(|r| r.eq_ignore_ascii_case(reference))
    {
        scope.relations.push(reference.into());
    }
    if let Some(alias) = alias {
        scope.aliases.push(alias.clone());
    }
    scope
        .occurrences
        .as_mut()
        .expect("owned join layout")
        .push((reference.into(), alias.cloned()));
}
// Find the source containing the LATERAL/APPLY opening, then accumulate only
// preceding inputs from its join ancestors. A group alias is visible after
// that group closes, not within its own definition.
fn joined_lateral_prefix(expr: &JoinExpr, at: usize) -> Option<Option<JoinExpr>> {
    match expr {
        JoinExpr::Relation { range, .. } => range.contains(&at).then_some(None),
        JoinExpr::Group { input, .. } => joined_lateral_prefix(input, at),
        JoinExpr::Join {
            left, right, kind, ..
        } => {
            if let Some(prefix) = joined_lateral_prefix(left, at) {
                return Some(prefix);
            }
            let prefix = joined_lateral_prefix(right, at)?;
            if matches!(kind, JoinKind::Right | JoinKind::Full) {
                return Some(prefix);
            }
            Some(Some(match prefix {
                Some(prefix) => joined_cross((**left).clone(), prefix),
                None => (**left).clone(),
            }))
        }
        _ => None,
    }
}
impl SqlParser {
    pub(crate) fn query_from_range_at_position(
        &self,
        tree: &Tree,
        source: &str,
        position: Position,
    ) -> Option<std::ops::Range<usize>> {
        joined_from(tree, Self::byte_offset_for_position(source, position), 0)
            .map(|n| n.byte_range())
    }
    pub(crate) fn lateral_source_scope(
        &self,
        tree: &Tree,
        source: &str,
        open: usize,
        start: usize,
    ) -> Option<RelationScope> {
        let from = joined_from(tree, open, start)?;
        let input = match joined_body(from, source, 0, &mut 256, self.sqlite_join_scopes) {
            Some(input) => joined_lateral_prefix(&input, open)?.unwrap_or(JoinExpr::Empty),
            None => JoinExpr::Unknown,
        };
        let mut scope = RelationScope {
            start,
            relations: Vec::new(),
            aliases: Vec::new(),
            occurrences: Some(Vec::new()),
            joined: None,
        };
        joined_occurrences(&input, &mut scope);
        scope.joined = Some(Box::new(input));
        Some(scope)
    }
    pub(crate) fn joined_row_dialect(&self) -> &'static str {
        if self.sqlite_join_scopes {
            return "sqlite";
        }
        match self.placeholder_dialect {
            crate::placeholder::SqlPlaceholderDialect::Postgres => "postgres",
            crate::placeholder::SqlPlaceholderDialect::Mysql => "mysql",
            crate::placeholder::SqlPlaceholderDialect::SqlServer => "sqlserver",
            _ => "generic",
        }
    }
    pub(crate) fn uses_joined_scope(&self, tree: &Tree, source: &str, position: Position) -> bool {
        self.has_joined_sources(tree, source, position)
            || Self::join_operand_range(tree, Self::byte_offset_for_position(source, position), 0)
                .is_some()
    }
    pub(crate) fn has_joined_sources(&self, tree: &Tree, source: &str, position: Position) -> bool {
        joined_from(tree, Self::byte_offset_for_position(source, position), 0)
            .is_some_and(joined_feature)
    }
    pub(crate) fn joined_source_scope(
        &self,
        tree: &Tree,
        source: &str,
        position: Position,
        start: usize,
    ) -> Option<RelationScope> {
        let cursor = Self::byte_offset_for_position(source, position);
        let from = joined_from(tree, cursor, start)?;
        if !joined_feature(from) {
            return None;
        }
        let mut node = tree.root_node().descendant_for_byte_range(
            cursor.min(source.len().saturating_sub(1)),
            cursor.min(source.len().saturating_sub(1)),
        );
        while let Some(current) = node {
            if current.kind() == "lateral_view"
                && current
                    .child_by_field_name("function")
                    .is_some_and(|n| n.start_byte() <= cursor && cursor <= n.end_byte())
            {
                return self.lateral_source_scope(tree, source, cursor, start);
            }
            if current.kind() == "from" {
                break;
            }
            node = current.parent();
        }
        let input = joined_body(from, source, 0, &mut 256, self.sqlite_join_scopes)
            .unwrap_or(JoinExpr::Unknown);
        let input = joined_condition(&input, cursor, self.sqlite_join_scopes)
            .map_or(input, |(input, _)| input);
        let mut scope = RelationScope {
            start,
            relations: Vec::new(),
            aliases: Vec::new(),
            occurrences: Some(Vec::new()),
            joined: None,
        };
        joined_occurrences(&input, &mut scope);
        scope.joined = Some(Box::new(input));
        Some(scope)
    }
}
