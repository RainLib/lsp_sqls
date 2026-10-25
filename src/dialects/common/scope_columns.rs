//! Column lookup uses query-block precedence and row-source identity, not the
//! synchronization order of catalog tables. Completion and hover share edges.
use super::items::create_column_item;
use super::references::{row_reference, table_matches};
use crate::identifier_search::column_match_rank;
use crate::{
    parser::{CompletionContext, RelationScope, SqlParser},
    schema::{Column, Schema, Table},
};
use std::{borrow::Cow, collections::HashSet};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position};
use tree_sitter::Tree;

struct ScopeBinding<'a> {
    table: Option<Cow<'a, Table>>,
    name: String,
    sql: String,
    aliased: bool,
}
impl ScopeBinding<'_> {
    fn matches(&self, schema: &Schema, qualifier: &str) -> bool {
        let single = SqlParser::identifier_qualifier(qualifier).is_none();
        let normalized = SqlParser::identifier_last_part(qualifier);
        if self.aliased {
            if !single {
                return false;
            }

            self.name == normalized
                || (!matches!(self.sql.as_bytes().first(), Some(b'"' | b'`' | b'['))
                    && self.name.eq_ignore_ascii_case(&normalized))
        } else {
            self.table
                .as_deref()
                .is_some_and(|table| table_matches(schema, table, qualifier))
                || single && self.name.eq_ignore_ascii_case(&normalized)
        }
    }
}
fn scope_bindings<'a>(
    scope: &RelationScope,
    schema: &'a Schema,
    manager: Option<&crate::schema::SchemaManager>,
) -> Vec<ScopeBinding<'a>> {
    if let Some(occurrences) = &scope.occurrences {
        return occurrences
            .iter()
            .map(|(reference, alias)| {
                let target = alias.as_ref().map_or(reference.as_str(), |alias| {
                    row_reference(schema, &scope.aliases, &alias.sql)
                });
                ScopeBinding {
                    table: crate::relation_bindings::projection_table(schema, manager, target),
                    name: alias.as_ref().map_or_else(
                        || SqlParser::identifier_last_part(reference),
                        |a| a.name.clone(),
                    ),
                    sql: alias
                        .as_ref()
                        .map_or_else(|| reference.clone(), |a| a.sql.clone()),
                    aliased: alias.is_some(),
                }
            })
            .collect();
    }
    let mut rows = Vec::new();
    for reference in &scope.relations {
        let aliases: Vec<_> = scope
            .aliases
            .iter()
            .filter(|alias| alias.relation.eq_ignore_ascii_case(reference))
            .collect();
        if aliases.is_empty() {
            rows.push(ScopeBinding {
                table: crate::relation_bindings::projection_table(schema, manager, reference),
                name: SqlParser::identifier_last_part(reference),
                sql: reference.clone(),
                aliased: false,
            });
        } else {
            for alias in aliases {
                let reference = row_reference(schema, &scope.aliases, &alias.sql);
                rows.push(ScopeBinding {
                    table: crate::relation_bindings::projection_table(schema, manager, reference),
                    name: alias.name.clone(),
                    sql: alias.sql.clone(),
                    aliased: true,
                });
            }
        }
    }
    rows
}

pub(crate) fn column_at_position(
    parser: &SqlParser,
    tree: &Tree,
    node: tree_sitter::Node<'_>,
    sql: &str,
    position: Position,
    schema: &Schema,
) -> Option<(String, Column)> {
    let name = SqlParser::identifier_last_part(&parser.node_text(node, sql));
    let qualifier = parser
        .column_qualifier_at_position(sql, position)
        .or_else(|| parser.get_table_name_for_column(node, sql));
    ColumnScopes::new(parser, tree, sql, position, schema, 0)
        .find_column(&name, qualifier.as_deref(), false)
        .map(|(table, column)| (table.name.clone(), column.clone()))
}

/// A request-local binding snapshot. Output inference reuses it for every
/// field in a projection instead of rescanning the original query per field.
pub(crate) struct ColumnScopes<'a> {
    schema: &'a Schema,
    scopes: Vec<Vec<ScopeBinding<'a>>>,
    outputs: Vec<Option<Table>>,
    joined: Vec<bool>,
}
impl<'a> ColumnScopes<'a> {
    /// AST consumers can bind ordered row occurrences through the same catalog
    /// and local-overlay rules as completion, without reparsing a statement.
    pub(crate) fn from_row_groups_with_metadata(
        schema: &'a Schema,
        groups: &[Vec<RelationScope>],
        unknown: &HashSet<(usize, usize)>,
        manager: Option<&crate::schema::SchemaManager>,
    ) -> Self {
        Self {
            schema,
            outputs: vec![None; groups.len()],
            joined: vec![false; groups.len()],
            scopes: groups
                .iter()
                .enumerate()
                .map(|(i, group)| {
                    group
                        .iter()
                        .enumerate()
                        .flat_map(|(j, row)| {
                            let mut bindings = scope_bindings(row, schema, manager);
                            if unknown.contains(&(i, j)) {
                                for b in &mut bindings {
                                    b.table = None;
                                }
                            }
                            bindings
                        })
                        .collect()
                })
                .collect(),
        }
    }
    pub(crate) fn row_columns(&self) -> Vec<Vec<Option<Vec<String>>>> {
        self.scopes
            .iter()
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        row.table
                            .as_deref()
                            .map(|t| t.columns.iter().map(|c| c.name.clone()).collect())
                    })
                    .collect()
            })
            .collect()
    }
    pub(crate) fn ambiguous(
        &self,
        name: &str,
        fold: bool,
        merged: &[std::collections::HashMap<String, usize>],
    ) -> bool {
        let key = if fold {
            name.to_ascii_lowercase()
        } else {
            name.into()
        };
        for (i, rows) in self.scopes.iter().enumerate() {
            if self.joined[i] {
                let Some(output) = &self.outputs[i] else {
                    return false;
                };
                let n = output
                    .columns
                    .iter()
                    .filter(|c| c.name == name || fold && c.name.eq_ignore_ascii_case(name))
                    .count();
                if n > 0 {
                    return n > 1;
                }
                continue;
            }
            let n = rows
                .iter()
                .filter(|r| {
                    r.table.as_deref().is_some_and(|t| {
                        t.columns
                            .iter()
                            .any(|c| c.name == name || fold && c.name.eq_ignore_ascii_case(name))
                    })
                })
                .count();
            let reduction = merged
                .get(i)
                .and_then(|m| m.get(&key))
                .copied()
                .unwrap_or(0);
            if n > 0 {
                return n.saturating_sub(reduction) > 1;
            }
            if rows.iter().any(|r| r.table.is_none()) {
                return false;
            }
        }
        false
    }

    pub(crate) fn new(
        parser: &SqlParser,
        tree: &Tree,
        sql: &str,
        position: Position,
        schema: &'a Schema,
        skip: usize,
    ) -> Self {
        Self::new_with_metadata(parser, tree, sql, position, schema, skip, None)
    }
    pub(crate) fn new_with_metadata(
        parser: &SqlParser,
        tree: &Tree,
        sql: &str,
        position: Position,
        schema: &'a Schema,
        skip: usize,
        manager: Option<&crate::schema::SchemaManager>,
    ) -> Self {
        let scopes = parser.relation_scopes_at_position(tree, sql, position);
        Self::from_scopes(parser, &scopes[skip.min(scopes.len())..], schema, manager)
    }
    fn from_scopes(
        parser: &SqlParser,
        input: &[RelationScope],
        schema: &'a Schema,
        manager: Option<&crate::schema::SchemaManager>,
    ) -> Self {
        let mut scopes = Vec::new();
        let mut outputs = Vec::new();
        let mut joined = Vec::new();
        for scope in input {
            if let Some(expr) = &scope.joined {
                let result = super::joined::resolve_joined_sources_with_metadata(
                    expr,
                    schema,
                    parser.joined_row_dialect(),
                    manager,
                );
                scopes.push(
                    result
                        .rows
                        .into_iter()
                        .map(|row| ScopeBinding {
                            name: row.alias.as_ref().map_or_else(
                                || SqlParser::identifier_last_part(&row.reference),
                                |a| a.name.clone(),
                            ),
                            sql: row
                                .alias
                                .as_ref()
                                .map_or_else(|| row.reference.clone(), |a| a.sql.clone()),
                            aliased: row.alias.is_some(),
                            table: row.table,
                        })
                        .collect(),
                );
                outputs.push(result.output);
                joined.push(true);
            } else {
                scopes.push(scope_bindings(scope, schema, manager));
                outputs.push(None);
                joined.push(false);
            }
        }
        Self {
            schema,
            scopes,
            outputs,
            joined,
        }
    }
    fn find_column(
        &self,
        name: &str,
        qualifier: Option<&str>,
        fold: bool,
    ) -> Option<(&Table, &Column)> {
        for (index, rows) in self.scopes.iter().enumerate() {
            if qualifier.is_none() && self.joined[index] {
                let output = self.outputs[index].as_ref()?;
                let mut columns = output
                    .columns
                    .iter()
                    .filter(|c| c.name == name || fold && c.name.eq_ignore_ascii_case(name));
                if let Some(column) = columns.next() {
                    if columns.next().is_some() {
                        return None;
                    }
                    let mut owners = rows.iter().filter_map(|r| r.table.as_deref()).filter(|t| {
                        t.columns
                            .iter()
                            .any(|c| c.name == name || fold && c.name.eq_ignore_ascii_case(name))
                    });
                    let first = owners.next();
                    let table = if owners.next().is_none() {
                        first.unwrap_or(output)
                    } else {
                        output
                    };
                    return Some((table, column));
                }
                continue;
            }
            let matching: Vec<_> = rows
                .iter()
                .filter(|row| qualifier.is_none_or(|q| row.matches(self.schema, q)))
                .collect();
            let mut columns = matching
                .iter()
                .filter_map(|row| row.table.as_deref())
                .flat_map(|table| {
                    table
                        .columns
                        .iter()
                        .filter(|column| {
                            column.name == name || fold && column.name.eq_ignore_ascii_case(name)
                        })
                        .map(move |column| (table, column))
                });
            if let Some(first) = columns.next() {
                return columns.next().is_none().then_some(first);
            }
            if qualifier.is_some() && !matching.is_empty()
                || qualifier.is_none() && rows.iter().any(|row| row.table.is_none())
            {
                return None;
            }
        }
        None
    }
    pub(crate) fn column_type(&self, name: &str, qualifier: Option<&str>) -> Option<String> {
        self.find_column(name, qualifier, true)
            .map(|(_, column)| column.data_type.clone())
    }
    pub(crate) fn qualified_star(&self, qualifier: &str) -> Vec<Column> {
        for rows in &self.scopes {
            let matching: Vec<_> = rows
                .iter()
                .filter(|row| row.matches(self.schema, qualifier))
                .collect();
            if !matching.is_empty() {
                return matching
                    .into_iter()
                    .filter_map(|row| row.table.as_deref())
                    .flat_map(|table| {
                        table
                            .columns
                            .iter()
                            .filter(|column| !column.hidden)
                            .cloned()
                    })
                    .collect();
            }
        }
        Vec::new()
    }
}

pub(crate) fn apply_scoped_column_completion(
    items: &mut Vec<CompletionItem>,
    parser: &SqlParser,
    tree: &Tree,
    sql: &str,
    position: Position,
    schema: &Schema,
    context: &CompletionContext,
) {
    let qualified = matches!(context, CompletionContext::TableColumn);
    let value_context = matches!(
        context,
        CompletionContext::SelectClause
            | CompletionContext::WhereClause
            | CompletionContext::HavingClause
            | CompletionContext::ExpressionValueClause
            | CompletionContext::CaseResultClause
            | CompletionContext::OrderByClause
            | CompletionContext::GroupByClause
    );
    if !qualified && !value_context {
        return;
    }
    let scopes = parser.relation_scopes_at_position(tree, sql, position);
    let bound = ColumnScopes::from_scopes(parser, &scopes, schema, None);
    if qualified {
        let Some(qualifier) = parser.column_qualifier_at_position(sql, position) else {
            return;
        };
        for (depth, rows) in bound.scopes.iter().enumerate() {
            let matching: Vec<_> = rows
                .iter()
                .filter(|row| row.matches(schema, &qualifier))
                .collect();
            if !matching.is_empty() {
                if bound.joined[depth] {
                    items.retain(|item| item.kind != Some(CompletionItemKind::FIELD));
                    if matching.len() == 1 {
                        if let Some(table) = matching[0].table.as_deref() {
                            for column in &table.columns {
                                if table
                                    .columns
                                    .iter()
                                    .filter(|c| c.name == column.name)
                                    .count()
                                    != 1
                                {
                                    continue;
                                }
                                items.push(create_column_item(column, None));
                            }
                        }
                    }
                }
                return;
            }
        }
        // Keep useful catalog member hints before an unfinished root FROM.
        if scopes.len() == 1 && scopes[0].start == 0 && scopes[0].relations.is_empty() {
            return;
        }
        items.retain(|item| item.kind != Some(CompletionItemKind::FIELD));
        return;
    }
    let Some(first) = scopes.first() else {
        items.retain(|item| item.kind != Some(CompletionItemKind::FIELD));
        return;
    };
    if first.start == 0 && first.occurrences.is_none() {
        return;
    }
    let prefix = parser
        .identifier_path_token(super::cursor::text_before_position(sql, position))
        .map(SqlParser::identifier_last_part)
        .unwrap_or_default()
        .to_lowercase();
    if bound.joined.first() == Some(&true) {
        items.retain(|item| item.kind != Some(CompletionItemKind::FIELD));
        let rows = &bound.scopes[0];
        if let Some(output) = &bound.outputs[0] {
            for column in &output.columns {
                if output
                    .columns
                    .iter()
                    .filter(|c| c.name == column.name)
                    .count()
                    != 1
                {
                    continue;
                }
                if column_match_rank(column, &prefix).is_none() {
                    continue;
                }
                items.push(create_column_item(column, None));
            }
        }
        for row in rows {
            if let Some(table) = row.table.as_deref() {
                for column in &table.columns {
                    let unique = bound.outputs[0].as_ref().is_some_and(|t| {
                        t.columns.iter().filter(|c| c.name == column.name).count() == 1
                    });
                    if unique
                        || table
                            .columns
                            .iter()
                            .filter(|c| c.name == column.name)
                            .count()
                            != 1
                        || column_match_rank(column, &prefix).is_none()
                    {
                        continue;
                    }
                    let mut item = create_column_item(column, Some(&row.name));
                    item.insert_text = Some(format!("{}.{}", row.sql, column.name));
                    items.push(item);
                }
            }
        }
    } else if first.occurrences.is_some() {
        // An AST-known operand keeps every row occurrence, even a bare table
        // joined to an alias of itself. Catalog-level de-duplication cannot
        // decide whether a bare field is a legal insert in that case.
        let rows = &bound.scopes[0];
        let mut counts = std::collections::HashMap::<String, usize>::new();
        for row in rows {
            if let Some(table) = row.table.as_deref() {
                let names: HashSet<_> = table
                    .columns
                    .iter()
                    .map(|c| c.name.to_ascii_lowercase())
                    .collect();
                for name in names {
                    *counts.entry(name).or_default() += 1;
                }
            }
        }
        items.retain(|item| item.kind != Some(CompletionItemKind::FIELD));
        for row in rows {
            if let Some(table) = row.table.as_deref() {
                for column in &table.columns {
                    let Some(rank) = column_match_rank(column, &prefix) else {
                        continue;
                    };
                    let ambiguous = counts
                        .get(&column.name.to_ascii_lowercase())
                        .copied()
                        .unwrap_or(0)
                        > 1;
                    let mut item =
                        create_column_item(column, ambiguous.then_some(row.name.as_str()));
                    if ambiguous {
                        item.insert_text = Some(format!("{}.{}", row.sql, column.name));
                    }
                    item.data = Some(
                        serde_json::json!({"oxide":{"kind":"column","queryLocal":crate::relation_bindings::query_local(table),"catalog":schema.catalog,"schema":schema.database,"table":table.name,"column":column.name,"searchComment":column.comment.as_deref().map(crate::identifier_search::comment_prefix)}}),
                    );
                    item.sort_text = Some(format!(
                        "{}:{rank}:{}",
                        if column.hidden { "3" } else { "2" },
                        column.name.to_ascii_lowercase()
                    ));
                    items.push(item);
                }
            }
        }
    }
    let mut blocked = HashSet::new();
    let mut inner_aliases = HashSet::new();
    for (depth, rows) in bound.scopes.iter().enumerate() {
        if depth == 0 && rows.is_empty() {
            items.retain(|item| item.kind != Some(CompletionItemKind::FIELD));
        }
        let mut names = HashSet::new();
        let mut merged_names = HashSet::new();
        if depth > 0 && bound.joined[depth] {
            if let Some(output) = &bound.outputs[depth] {
                for column in &output.columns {
                    let key = column.name.to_ascii_lowercase();
                    if blocked.contains(&key)
                        || column_match_rank(column, &prefix).is_none()
                        || output
                            .columns
                            .iter()
                            .filter(|c| c.name == column.name)
                            .count()
                            != 1
                    {
                        continue;
                    }
                    let owners = rows
                        .iter()
                        .filter_map(|r| r.table.as_deref())
                        .filter(|t| t.columns.iter().any(|c| c.name == column.name))
                        .count();
                    if owners < 2 {
                        continue;
                    }
                    let mut item = create_column_item(column, None);
                    item.detail = Some(format!(
                        "Outer column: {} ({})",
                        column.name, column.data_type
                    ));
                    item.sort_text = Some(format!("1:{depth:02}:{}", key));
                    items.push(item);
                    merged_names.insert(key);
                }
            }
        }
        for row in rows {
            if let Some(table) = row.table.as_deref() {
                for column in &table.columns {
                    let key = column.name.to_ascii_lowercase();
                    names.insert(key.clone());
                    if depth == 0
                        || blocked.contains(&key)
                        || merged_names.contains(&key)
                        || column_match_rank(column, &prefix).is_none()
                    {
                        continue;
                    }
                    let shadowed = inner_aliases.contains(&row.name.to_ascii_lowercase());
                    let mut item =
                        create_column_item(column, (!shadowed).then_some(row.name.as_str()));
                    if !shadowed {
                        item.insert_text = Some(format!("{}.{}", row.sql, column.name));
                    }
                    item.detail = Some(format!(
                        "Outer column: {}.{} ({})",
                        table.name, column.name, column.data_type
                    ));
                    item.sort_text =
                        Some(format!("1:{depth:02}:{}", column.name.to_ascii_lowercase()));
                    item.data = Some(
                        serde_json::json!({"oxide":{"kind":"column","queryLocal":crate::relation_bindings::query_local(table),"catalog":schema.catalog,"schema":schema.database,"table":table.name,"column":column.name,"searchComment":column.comment.as_deref().map(crate::identifier_search::comment_prefix)}}),
                    );
                    items.push(item);
                }
            }
        }
        blocked.extend(names);
        inner_aliases.extend(rows.iter().map(|row| row.name.to_ascii_lowercase()));
        if rows.iter().any(|row| row.table.is_none()) {
            break;
        }
    }
}
