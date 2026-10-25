//! Scope-bound projection inputs. Output aliases never select a type from an
//! unrelated catalog column with the same spelling.
use super::{
    read_local_identifier_parts_after, read_local_identifier_path_after, schema_table_matches,
    table_functions,
};
use crate::{
    parser::SqlParser,
    schema::{Column, Schema, Table},
};

pub(super) fn row_source_schema(
    query: &str,
    upper: &str,
    schema: &Schema,
    uri: &str,
    cursor: usize,
    line: u32,
    dialect: Option<&str>,
) -> Option<Schema> {
    let mut tables = super::correlated_tables_before_cursor(query, upper, schema, uri, cursor);
    tables.extend(table_functions::declared_function_tables(
        query, upper, uri, cursor, dialect,
    ));
    if tables.is_empty() {
        return None;
    }
    let mut augmented = schema.clone();
    for mut table in tables {
        for location in table.source_location.iter_mut().chain(
            table
                .columns
                .iter_mut()
                .filter_map(|column| column.source_location.as_mut()),
        ) {
            location.1 = location.1.saturating_add(line.saturating_sub(1));
        }
        super::install_local_table(&mut augmented, table);
    }
    Some(augmented)
}

pub(super) struct ProjectionSources<'a> {
    variables: Option<super::variables::Origin<'a>>,
    schema: &'a Schema,
    routine_manager: Option<&'a crate::schema::SchemaManager>,
    rows: Vec<(Option<String>, std::borrow::Cow<'a, Table>)>,
    tree: Option<tree_sitter::Tree>,
    joined_model: bool,
    joined_output: Option<Table>,
    mysql_matches: std::collections::HashSet<(usize, usize)>,
    outer: Option<crate::dialects::common::ColumnScopes<'a>>,
    unresolved: Vec<String>,
    subquery_types:
        std::collections::HashMap<(usize, usize), Vec<super::expression_types::ExpressionType>>,
}

impl<'a> ProjectionSources<'a> {
    pub(super) fn new(
        query: &str,
        upper: &str,
        schema: &'a Schema,
        cursor: usize,
        dialect: Option<&str>,
        derived_names: &[String],
        environment: super::query_output::Environment<'a>,
    ) -> Self {
        let position = crate::position::byte_position_at_end(&query[..cursor]);
        let aliases = SqlParser::relation_aliases_at_position(query, position);
        let placeholder_dialect = crate::placeholder::SqlPlaceholderDialect::for_name(dialect);
        let mut parser = SqlParser::new_with_placeholder_dialect(placeholder_dialect);
        let mut rows = Vec::new();
        let mut unresolved = Vec::new();
        let tree = environment
            .origin
            .and_then(|origin| origin.exact_tree(query).cloned())
            .or_else(|| parser.parse_syntax_tree(query));
        if let Some(tree) = &tree {
            for reference in parser.extract_row_sources_at_position(tree, query, position) {
                let mut bindings: Vec<_> = aliases
                    .iter()
                    .filter(|alias| alias.relation.eq_ignore_ascii_case(&reference))
                    .collect();
                // Named function shapes retain discovery order, including two
                // different invocations of the same built-in function.
                bindings.sort_by_key(|alias| {
                    schema
                        .tables
                        .iter()
                        .position(|table| {
                            table.object_type.as_deref() == Some("TABLE FUNCTION")
                                && table.name.eq_ignore_ascii_case(&alias.name)
                        })
                        .unwrap_or(usize::MAX)
                });
                if bindings.is_empty() {
                    if let Some(table) = crate::relation_bindings::projection_table(
                        schema,
                        environment.routines,
                        &reference,
                    ) {
                        let alias = (table.object_type.as_deref() == Some("DERIVED TABLE"))
                            .then(|| table.name.clone());
                        rows.push((alias, table));
                    } else {
                        unresolved.push(SqlParser::identifier_path_from_parts(&[
                            SqlParser::identifier_last_part(&reference),
                        ]));
                    }
                } else {
                    for alias in bindings {
                        if let Some(table) = function_alias(schema, &alias.name)
                            .map(std::borrow::Cow::Borrowed)
                            .or_else(|| {
                                crate::relation_bindings::projection_table(
                                    schema,
                                    environment.routines,
                                    &reference,
                                )
                            })
                        {
                            rows.push((Some(alias.name.clone()), table));
                        } else {
                            unresolved.push(SqlParser::identifier_path_from_parts(
                                std::slice::from_ref(&alias.name),
                            ));
                        }
                    }
                }
            }
        }
        for name in derived_names {
            if let Some(table) = schema.tables.iter().find(|table| {
                table.name == *name && table.object_type.as_deref() == Some("DERIVED TABLE")
            }) {
                if !rows
                    .iter()
                    .any(|(_, existing)| std::ptr::eq(existing.as_ref(), table))
                {
                    rows.push((Some(name.clone()), std::borrow::Cow::Borrowed(table)));
                }
            }
        }
        let order = input_order(query, upper, cursor);
        rows.sort_by_key(|(alias, table)| {
            order
                .iter()
                .find(|(reference, _, candidate_alias)| {
                    alias.as_ref().map_or_else(
                        || super::schema_table_matches(schema, reference, table),
                        |alias| {
                            candidate_alias
                                .as_ref()
                                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(alias))
                        },
                    )
                })
                .map_or(usize::MAX, |(_, offset, _)| *offset)
        });
        let mut joined_model = false;
        let mut joined_output = None;
        if let Some(expr) = tree
            .as_ref()
            .and_then(|t| {
                parser
                    .relation_scopes_at_position(t, query, position)
                    .into_iter()
                    .next()
            })
            .and_then(|s| s.joined)
        {
            let result = crate::dialects::common::resolve_joined_sources_with_metadata(
                &expr,
                schema,
                dialect.unwrap_or("generic"),
                environment.routines,
            );
            rows.clear();
            unresolved.clear();
            joined_model = true;
            joined_output = result.output;
            for row in result.rows {
                let alias = row.alias.map(|a| a.name);
                if let Some(table) = row.table {
                    rows.push((alias, table));
                } else {
                    unresolved.push(
                        alias.unwrap_or_else(|| SqlParser::identifier_last_part(&row.reference)),
                    );
                }
            }
        }
        let mysql_matches = SqlParser::match_against_expressions(query)
            .into_iter()
            .map(|expression| (expression.start, expression.end))
            .collect();
        Self {
            variables: environment.variables,
            subquery_types: Default::default(),
            routine_manager: environment.routines,
            schema,
            rows,
            tree,
            joined_model,
            joined_output,
            mysql_matches,
            outer: environment
                .origin
                .and_then(|origin| origin.outer_columns(query, cursor, &parser, schema)),
            unresolved,
        }
    }

    pub(super) fn infer_scalar_outputs(
        &mut self,
        context: &super::query_output::Context<'_>,
        query: &str,
        upper: &str,
        projection: std::ops::Range<usize>,
        depth: usize,
        budget: &mut usize,
    ) {
        self.subquery_types = super::scalar_queries::infer(
            context,
            query,
            upper,
            self.tree.as_ref(),
            projection,
            depth,
            budget,
        );
    }

    pub(super) fn scalar_type(
        &self,
        start: usize,
        end: usize,
    ) -> Option<super::expression_types::ExpressionType> {
        let columns = self.subquery_types.get(&(start, end))?;
        (columns.len() == 1 && columns[0].data_type != "unknown").then(|| columns[0].clone())
    }

    pub(super) fn variable_type(
        &self,
        query: &str,
        at: usize,
        name: &str,
    ) -> Option<super::expression_types::ExpressionType> {
        self.variables?.lookup(query, at, name)
    }

    pub(super) fn row_types(
        &self,
        start: usize,
        end: usize,
    ) -> Option<&[super::expression_types::ExpressionType]> {
        self.subquery_types.get(&(start, end)).map(Vec::as_slice)
    }

    fn matching_rows(&self, qualifier: Option<&str>) -> Vec<&Table> {
        self.rows
            .iter()
            .filter(|(alias, table)| match qualifier {
                None => true,
                Some(qualifier) => alias.as_ref().map_or_else(
                    || schema_table_matches(self.schema, qualifier, table),
                    |alias| {
                        SqlParser::identifier_qualifier(qualifier).is_none()
                            && alias
                                .eq_ignore_ascii_case(&SqlParser::identifier_last_part(qualifier))
                    },
                ),
            })
            .map(|(_, table)| table.as_ref())
            .collect()
    }

    pub(super) fn star_projection(
        &self,
        query: &str,
        start: usize,
        end: usize,
    ) -> Option<Vec<Column>> {
        let term = self.projection_term(query, start, end)?;
        let value = term.child_by_field_name("value")?;
        if value.kind() != "all_fields" {
            return None;
        }
        let qualifier = value
            .named_children(&mut value.walk())
            .find(|node| node.kind() == "object_reference")
            .and_then(|node| node.utf8_text(query.as_bytes()).ok());
        Some(self.star(qualifier))
    }

    pub(super) fn star(&self, qualifier: Option<&str>) -> Vec<Column> {
        if qualifier.is_none() && self.joined_model {
            return self
                .joined_output
                .as_ref()
                .map(|t| t.columns.iter().filter(|c| !c.hidden).cloned().collect())
                .unwrap_or_default();
        }
        let qualifier = qualifier.map(SqlParser::normalize_identifier);
        let rows = self.matching_rows(qualifier.as_deref());
        if qualifier.is_none() && !self.unresolved.is_empty() {
            return Vec::new();
        }
        if rows.is_empty() {
            if let Some(qualifier) = qualifier.as_deref().filter(|q| {
                !self
                    .unresolved
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(q))
            }) {
                if let Some(outer) = &self.outer {
                    return outer.qualified_star(qualifier);
                }
            }
        }
        rows.into_iter()
            .flat_map(|table| {
                table
                    .columns
                    .iter()
                    .filter(|column| !column.hidden)
                    .cloned()
            })
            .collect()
    }

    pub(super) fn expression_type(
        &self,
        query: &str,
        start: usize,
        end: usize,
        dialect: Option<&str>,
    ) -> Option<super::expression_types::ExpressionType> {
        let node = self.projection_term(query, start, end)?;
        super::expression_types::infer(node.child_by_field_name("value")?, query, self, dialect, 0)
    }

    pub(super) fn expression_name(&self, query: &str, start: usize, end: usize) -> Option<String> {
        let node = self.projection_term(query, start, end)?;
        let name = node.child_by_field_name("alias").or_else(|| {
            let value = node.child_by_field_name("value")?;
            (value.kind() == "field")
                .then(|| value.child_by_field_name("name"))
                .flatten()
        })?;
        super::read_local_identifier_after(query, name.start_byte()).map(|(name, _, _)| name)
    }

    /// Inherit metadata only through an unambiguous direct field projection.
    /// Computed values do not borrow a same-named catalog field's comment.
    pub(super) fn expression_comment(
        &self,
        query: &str,
        start: usize,
        end: usize,
    ) -> Option<String> {
        let node = self
            .projection_term(query, start, end)?
            .child_by_field_name("value")?;
        if node.kind() != "field" {
            return None;
        }
        let expression = query.get(node.byte_range())?;
        let (parts, _, end) = read_local_identifier_parts_after(expression, 0)?;
        if !expression[end..].trim().is_empty() {
            return None;
        }
        let (name, qualifier) = parts.split_last()?;
        if qualifier.is_empty() && self.joined_model {
            let output = self.joined_output.as_ref()?;
            let mut columns = output
                .columns
                .iter()
                .filter(|c| !c.hidden && c.name.eq_ignore_ascii_case(name));
            let column = columns.next()?;
            return columns
                .next()
                .is_none()
                .then(|| column.comment.clone())
                .flatten();
        }
        let qualifier =
            (!qualifier.is_empty()).then(|| SqlParser::identifier_path_from_parts(qualifier));
        let rows = self.matching_rows(qualifier.as_deref());
        let mut columns = rows
            .iter()
            .flat_map(|table| &table.columns)
            .filter(|c| !c.hidden && c.name.eq_ignore_ascii_case(name));
        let column = columns.next()?;
        columns
            .next()
            .is_none()
            .then(|| column.comment.clone())
            .flatten()
    }

    pub(super) fn comparison_ranges(
        &self,
        operator: usize,
    ) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
        super::provenance::comparison_ranges(self.tree.as_ref()?, operator)
    }

    pub(super) fn argument_type(
        &self,
        query: &str,
        content: &str,
        range: std::ops::Range<usize>,
        dialect: Option<&str>,
    ) -> Option<super::expression_types::ExpressionType> {
        let node = self.argument_node(content, range)?;
        super::expression_types::infer(node, query, self, dialect, 0)
    }
    pub(super) fn continuation_pattern(
        &self,
        query: &str,
        content: &str,
        range: std::ops::Range<usize>,
        dialect: &str,
    ) -> Option<super::expression_types::PatternContinuation> {
        super::expression_types::pattern_continuation(
            self.argument_node(content, range)?,
            query,
            self,
            dialect,
        )
    }
    fn argument_node(
        &self,
        content: &str,
        range: std::ops::Range<usize>,
    ) -> Option<tree_sitter::Node<'_>> {
        let start = range.start
            + content
                .get(range.clone())?
                .find(|ch: char| !ch.is_whitespace())?;
        let mut node = self
            .tree
            .as_ref()?
            .root_node()
            .named_descendant_for_byte_range(start, start)?;
        while let Some(parent) = node
            .parent()
            .filter(|parent| parent.start_byte() >= start && parent.end_byte() <= range.end)
        {
            node = parent;
        }
        if node.start_byte() != start
            || node.end_byte() > range.end
            || !content.get(node.end_byte()..range.end)?.trim().is_empty()
        {
            return None;
        }
        Some(node)
    }

    pub(super) fn invocation_arguments(
        &self,
        opening: usize,
    ) -> Option<(Vec<std::ops::Range<usize>>, bool)> {
        let mut node = self
            .tree
            .as_ref()?
            .root_node()
            .descendant_for_byte_range(opening, opening)?;
        loop {
            if node.kind() == "invocation"
                && node
                    .children(&mut node.walk())
                    .any(|child| child.kind() == "(" && child.start_byte() == opening)
            {
                break;
            }
            node = node.parent()?;
        }
        if node.has_error()
            || node.end_byte().saturating_sub(opening) > 64 * 1024
            || !node
                .children(&mut node.walk())
                .any(|child| child.kind() == ")")
        {
            return None;
        }
        let ranges = node
            .children_by_field_name("parameter", &mut node.walk())
            .map(|child| child.byte_range())
            .collect::<Vec<_>>();
        let explicit = node
            .children(&mut node.walk())
            .any(|child| child.kind() == "keyword_variadic");
        (ranges.len() <= 256).then_some((ranges, explicit))
    }

    fn projection_term(
        &self,
        query: &str,
        start: usize,
        end: usize,
    ) -> Option<tree_sitter::Node<'_>> {
        let start = super::skip_sql_trivia(query, start);
        if start >= end {
            return None;
        }
        let mut node = self
            .tree
            .as_ref()?
            .root_node()
            .descendant_for_byte_range(start, start)?;
        loop {
            if node.kind() == "term"
                && node.start_byte() == start
                && node.end_byte() <= end
                && super::skip_sql_trivia(query, node.end_byte()) >= end
            {
                return Some(node);
            }
            node = node.parent()?;
            if node.start_byte() < start || node.end_byte() > end {
                return None;
            }
        }
    }

    pub(super) fn is_mysql_match(&self, start: usize, end: usize) -> bool {
        self.mysql_matches.contains(&(start, end))
    }

    pub(super) fn server_version(&self) -> Option<(u32, u32)> {
        self.schema.server_version_tuple()
    }

    pub(super) fn routine_metadata(
        &self,
        identity: &crate::server::routine_identity::RoutineIdentity,
        dialect: &str,
    ) -> Option<std::borrow::Cow<'a, Schema>> {
        if identity.namespace_matches(self.schema, dialect) {
            Some(std::borrow::Cow::Borrowed(self.schema))
        } else {
            identity.schema(Some(self.schema), self.routine_manager?, dialect)
        }
    }

    pub(super) fn column_type(&self, expression: &str) -> Option<String> {
        let (parts, _, end) = read_local_identifier_parts_after(expression, 0)?;
        let tail = expression[end..].trim();
        let alias = if tail
            .get(..2)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("AS"))
            && tail
                .get(2..)
                .and_then(|suffix| suffix.chars().next())
                .is_some_and(char::is_whitespace)
        {
            tail[2..].trim()
        } else {
            tail
        };
        if !alias.is_empty() {
            let (_, _, end) = read_local_identifier_path_after(alias, 0)?;
            if !alias[end..].trim().is_empty() {
                return None;
            }
        }
        let (name, qualifier) = parts.split_last()?;
        let qualifier =
            (!qualifier.is_empty()).then(|| SqlParser::identifier_path_from_parts(qualifier));
        if qualifier.is_none() && self.joined_model {
            let output = self.joined_output.as_ref()?;
            let matches: Vec<_> = output
                .columns
                .iter()
                .filter(|c| !c.hidden && c.name.eq_ignore_ascii_case(name))
                .collect();
            return (matches.len() == 1).then(|| matches[0].data_type.clone());
        }
        let rows = self.matching_rows(qualifier.as_deref());
        if qualifier.is_none() && !self.unresolved.is_empty() {
            return None;
        }
        let matches: Vec<_> = rows
            .iter()
            .flat_map(|table| {
                table
                    .columns
                    .iter()
                    .filter(|column| !column.hidden && column.name.eq_ignore_ascii_case(name))
            })
            .collect();
        if !matches.is_empty() {
            return (matches.len() == 1).then(|| matches[0].data_type.clone());
        }
        if qualifier.is_some()
            && (!rows.is_empty()
                || self.unresolved.iter().any(|row| {
                    qualifier
                        .as_deref()
                        .is_some_and(|q| row.eq_ignore_ascii_case(q))
                }))
            || qualifier.is_none() && !self.unresolved.is_empty()
        {
            return None;
        }
        self.outer.as_ref()?.column_type(name, qualifier.as_deref())
    }
}

fn function_alias<'a>(schema: &'a Schema, alias: &str) -> Option<&'a Table> {
    schema.tables.iter().find(|table| {
        matches!(
            table.object_type.as_deref(),
            Some("TABLE FUNCTION" | "CORRELATED TABLE")
        ) && table.name.eq_ignore_ascii_case(alias)
    })
}

// Preserve FROM/JOIN order when physical and derived inputs are mixed. Names
// are read only from the current query block, never by a substring search.
fn input_order(query: &str, upper: &str, cursor: usize) -> Vec<(String, usize, Option<String>)> {
    let mut entries = Vec::new();
    for offset in super::local_relation_source_starts(upper) {
        if !super::scope_contains(upper, offset, cursor) {
            continue;
        }
        let mut start = super::skip_sql_trivia(query, offset);
        for modifier in ["LATERAL", "ONLY"] {
            if upper[start..].starts_with(modifier)
                && super::local_keyword_boundary(upper, start, modifier)
            {
                start = super::skip_sql_trivia(query, start + modifier.len());
            }
        }
        let (reference, mut end) = if query[start..].starts_with('(') {
            let Some(close) = super::matching_parenthesis_in_searchable(upper, start) else {
                continue;
            };
            (String::new(), close + 1)
        } else {
            let Some((reference, _, end)) = super::read_local_identifier_path_after(query, start)
            else {
                continue;
            };
            (reference, end)
        };
        end = super::skip_sql_trivia(query, end);
        if query[end..].starts_with('(') {
            let Some(close) = super::matching_parenthesis_in_searchable(upper, end) else {
                continue;
            };
            end = super::skip_sql_trivia(query, close + 1);
        }
        if upper[end..].starts_with("WITH") && super::local_keyword_boundary(upper, end, "WITH") {
            let next = super::skip_sql_trivia(query, end + 4);
            if upper[next..].starts_with("ORDINALITY")
                && super::local_keyword_boundary(upper, next, "ORDINALITY")
            {
                end = super::skip_sql_trivia(query, next + 10);
            }
        }
        if upper[end..].starts_with("AS") && super::local_keyword_boundary(upper, end, "AS") {
            end = super::skip_sql_trivia(query, end + 2);
        }
        let alias = super::read_local_identifier_after(query, end)
            .map(|(alias, _, _)| alias)
            .filter(|alias| !crate::token::Keywords::is_keyword(alias));
        entries.push((reference, offset, alias));
    }
    entries
}
