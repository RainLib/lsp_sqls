//! SQL-local relation shapes. No transport or editor state is owned here.
use super::completion_context::position_to_byte_offset;
use super::schema_resolution::schema_table_matches;
use crate::{
    parser::SqlParser,
    schema::{Column, Schema, Table},
    token::Keywords,
};
use std::collections::HashMap;

mod argument_types;
mod column_constraints;
mod expression_types;
mod hive_types;
mod lateral_views;
pub(super) use argument_types::{
    infer_comparison_left_type, infer_comparison_value_type, infer_continuation_type,
    infer_routine_argument_types,
};
#[cfg(test)]
mod output_tests;
#[cfg(test)]
mod pattern_escape_tests;
#[cfg(test)]
mod pattern_tests;
#[cfg(test)]
mod predicate_tests;
mod projection;
mod provenance;
mod qualify;
#[cfg(test)]
mod quantified_tests;
mod query_output;
pub(super) use qualify::add_qualify_completions;
mod recursive_ctes;
mod scalar_queries;
mod with_queries;
#[cfg(test)]
pub(super) use query_output::infer_query_output_columns;
mod syntax;
mod table_functions;
mod table_variables;
mod variables;
pub(super) use syntax::*;
use tower_lsp::lsp_types::Position;

pub(super) const LOCAL_RELATION_SCAN_MAX_BYTES: usize = 512 * 1024;

#[cfg(test)]
pub(super) fn augment_schema_with_local_relations(
    schema: Schema,
    text: &str,
    position: Position,
    uri: &str,
    dialect: Option<&str>,
) -> Schema {
    augment_schema_with_local_relations_and_routines(schema, text, position, uri, dialect, None)
}

pub(super) fn augment_schema_with_local_relations_and_routines(
    mut schema: Schema,
    text: &str,
    position: Position,
    uri: &str,
    dialect: Option<&str>,
    routines: Option<&crate::schema::SchemaManager>,
) -> Schema {
    let dialect =
        dialect.map(
            |name| match crate::placeholder::SqlPlaceholderDialect::for_name(Some(name)) {
                crate::placeholder::SqlPlaceholderDialect::Postgres => "postgres",
                crate::placeholder::SqlPlaceholderDialect::Mysql => "mysql",
                crate::placeholder::SqlPlaceholderDialect::SqlServer => "sqlserver",
                _ => name,
            },
        );
    let cursor = position_to_byte_offset(text, position).min(text.len());
    if text.len() > LOCAL_RELATION_SCAN_MAX_BYTES {
        return schema;
    }
    let searchable = SqlParser::mask_sql_noise(text).to_ascii_uppercase();
    let mut local_relations = temporary_tables_before_cursor(
        &text[..cursor],
        &searchable[..cursor],
        &schema,
        uri,
        dialect,
        routines,
    );
    local_relations.extend(table_variables::tables(text, cursor, uri, dialect));
    let start = SqlParser::previous_statement_start(&searchable, cursor);
    let end = SqlParser::next_statement_end(&searchable, cursor);
    // Preserve byte offsets and newlines for navigation into the original URI.
    let mut statement = text.as_bytes().to_vec();
    for (index, byte) in statement.iter_mut().enumerate() {
        if (index < start || index >= end) && *byte != b'\n' {
            *byte = b' ';
        }
    }
    let statement = String::from_utf8(statement).expect("masked SQL remains UTF-8");
    let searchable_upper = SqlParser::mask_sql_noise(&statement).to_ascii_uppercase();
    let visible = statement.as_str();
    let variables = variables::Snapshot::new(text, cursor, dialect, None);
    let environment = query_output::Environment {
        variables: variables
            .as_ref()
            .and_then(|snapshot| snapshot.origin(visible)),
        routines,
        origin: None,
    };
    let mut base_schema = schema.clone();
    for table in &local_relations {
        install_local_table(&mut base_schema, table.clone());
    }
    local_relations.extend(cte_tables_before_cursor(
        visible,
        &searchable_upper,
        &base_schema,
        uri,
        cursor,
        dialect,
        environment,
    ));
    for table in &local_relations {
        install_local_table(&mut base_schema, table.clone());
    }
    local_relations.extend(derived_tables_before_cursor(
        visible,
        &searchable_upper,
        &base_schema,
        uri,
        cursor,
        dialect,
        environment,
    ));
    let mut relation_schema = schema.clone();
    for table in &local_relations {
        install_local_table(&mut relation_schema, table.clone());
    }
    local_relations.extend(correlated_tables_before_cursor(
        visible,
        &searchable_upper,
        &relation_schema,
        uri,
        cursor,
    ));
    local_relations.extend(table_functions::declared_function_tables(
        visible,
        &searchable_upper,
        uri,
        cursor,
        dialect,
    ));

    // Apply in discovery order so the most local shape wins. A correlation
    // column list shadows remote/CTE names positionally, and a CTE shadows a
    // temporary relation with the same name.
    for table in local_relations {
        install_local_table(&mut schema, table);
    }
    for table in lateral_views::tables(visible, &schema, uri, cursor, dialect, environment) {
        install_local_table(&mut schema, table);
    }
    schema
}

pub(super) fn temporary_tables_before_cursor(
    source: &str,
    upper: &str,
    schema: &Schema,
    uri: &str,
    dialect: Option<&str>,
    routines: Option<&crate::schema::SchemaManager>,
) -> Vec<Table> {
    let mut tables = HashMap::<String, (usize, Table)>::new();
    for keyword in ["CREATE TEMP TABLE", "CREATE TEMPORARY TABLE"] {
        let mut search = 0;
        while let Some(relative) = upper[search..].find(keyword) {
            let start = search + relative;
            let after_keyword = start + keyword.len();
            let name_position =
                skip_optional_keyword_sequence(source, after_keyword, &["IF", "NOT", "EXISTS"]);
            let Some((name, name_start, name_end)) =
                read_local_identifier_after(source, name_position)
            else {
                search = after_keyword;
                continue;
            };
            let definition_start = skip_local_whitespace(source, name_end);
            let source_line = source[..name_start].matches('\n').count() as u32 + 1;
            let (columns, statement_end) = if source[definition_start..].starts_with('(') {
                let Some(closing) = matching_parenthesis_in_searchable(upper, definition_start)
                else {
                    search = definition_start + 1;
                    continue;
                };
                (
                    split_top_level_ranges_in_searchable(upper, definition_start + 1, closing)
                        .into_iter()
                        .filter_map(|(column_start, column_end)| {
                            parse_temporary_column(
                                &source[column_start..column_end],
                                uri,
                                source[..column_start].matches('\n').count() as u32 + 1,
                            )
                        })
                        .collect(),
                    closing + 1,
                )
            } else if upper[definition_start..].starts_with("AS")
                && local_keyword_boundary(upper, definition_start, "AS")
            {
                let query_start = skip_local_whitespace(source, definition_start + "AS".len());
                let statement_end = upper[query_start..]
                    .find(';')
                    .map_or(source.len(), |relative| query_start + relative);
                (
                    query_output::infer_query_output_columns_with_routines(
                        &source[query_start..statement_end],
                        schema,
                        uri,
                        source_line,
                        dialect,
                        routines,
                    ),
                    statement_end,
                )
            } else {
                search = name_end;
                continue;
            };
            let table =
                local_relation_table(name.clone(), "TEMPORARY TABLE", columns, uri, source_line);
            tables.insert(name.to_ascii_lowercase(), (start, table));
            search = statement_end;
        }
    }

    for keyword in ["DROP TABLE", "DROP TEMP TABLE", "DROP TEMPORARY TABLE"] {
        let mut search = 0;
        while let Some(relative) = upper[search..].find(keyword) {
            let start = search + relative;
            let after_keyword = start + keyword.len();
            let name_position =
                skip_optional_keyword_sequence(source, after_keyword, &["IF", "EXISTS"]);
            if let Some((name, _, name_end)) = read_local_identifier_after(source, name_position) {
                let normalized = name.to_ascii_lowercase();
                if tables
                    .get(&normalized)
                    .is_some_and(|(created_at, _)| *created_at < start)
                {
                    tables.remove(&normalized);
                }
                search = name_end;
            } else {
                search = after_keyword;
            }
        }
    }

    let mut tables = tables.into_values().collect::<Vec<_>>();
    tables.sort_by_key(|(offset, _)| *offset);
    tables.into_iter().map(|(_, table)| table).collect()
}

pub(super) fn parse_temporary_column(definition: &str, uri: &str, line: u32) -> Option<Column> {
    parse_temporary_column_with_mask(
        definition,
        &SqlParser::mask_sql_noise(definition),
        uri,
        line,
    )
}

fn parse_temporary_column_with_mask(
    definition: &str,
    masked: &str,
    uri: &str,
    line: u32,
) -> Option<Column> {
    let (name, name_start, name_end) = read_local_identifier_after(definition, 0)?;
    if !matches!(definition.as_bytes()[name_start], b'"' | b'`' | b'[')
        && matches!(
            name.to_ascii_uppercase().as_str(),
            "PRIMARY" | "UNIQUE" | "CONSTRAINT" | "CHECK" | "FOREIGN" | "EXCLUDE"
        )
    {
        return None;
    }
    let (data_type, _, type_end) = read_local_identifier_after(definition, name_end)
        .unwrap_or_else(|| ("unknown".to_string(), name_end, name_end));
    let flags = column_constraints::flags(definition, masked, type_end);
    Some(Column {
        name,
        data_type,
        nullable: !flags.not_null,
        primary_key: flags.primary_key,
        unique: flags.unique,
        indexed: false,
        default_value: None,
        auto_increment: false,
        generated: false,
        hidden: false,
        comment: None,
        source_location: Some((uri.to_string(), line)),
    })
}

fn cte_tables_before_cursor(
    source: &str,
    upper: &str,
    schema: &Schema,
    uri: &str,
    position: usize,
    dialect: Option<&str>,
    environment: query_output::Environment<'_>,
) -> Vec<Table> {
    let mut tables = Vec::new();
    let mut working_schema = schema.clone();
    let mut search = 0;
    let mut parsed_origin = None;
    while let Some(relative) = upper[search..].find("WITH") {
        let with_start = search + relative;
        if with_start > position
            || !scope_contains(upper, with_start, position)
            || !local_keyword_boundary(upper, with_start, "WITH")
        {
            search = with_start + "WITH".len();
            continue;
        }
        let mut cursor = skip_sql_trivia(source, with_start + "WITH".len());
        let recursive = upper[cursor..].starts_with("RECURSIVE")
            && local_keyword_boundary(upper, cursor, "RECURSIVE");
        if recursive {
            cursor = skip_sql_trivia(source, cursor + "RECURSIVE".len());
        }
        while let Some((name, name_start, name_end)) = read_local_identifier_after(source, cursor) {
            cursor = skip_sql_trivia(source, name_end);
            let mut explicit_columns = Vec::new();
            if source[cursor..].starts_with('(') {
                let Some(closing) = matching_parenthesis_in_searchable(upper, cursor) else {
                    break;
                };
                explicit_columns = split_top_level_ranges_in_searchable(upper, cursor + 1, closing)
                    .into_iter()
                    .filter_map(|(start, _)| {
                        read_local_identifier_after(source, start).map(|(name, _, _)| name)
                    })
                    .collect();
                cursor = skip_sql_trivia(source, closing + 1);
            }
            if !upper[cursor..].starts_with("AS") || !local_keyword_boundary(upper, cursor, "AS") {
                break;
            }
            cursor = skip_sql_trivia(source, cursor + "AS".len());
            cursor = skip_optional_keyword_sequence(upper, cursor, &["NOT", "MATERIALIZED"]);
            cursor = skip_optional_keyword_sequence(upper, cursor, &["MATERIALIZED"]);
            if !source[cursor..].starts_with('(') {
                break;
            }
            let closing = matching_parenthesis_in_searchable(upper, cursor).unwrap_or(source.len());
            let inside_body = cursor < position && position <= closing;
            if position <= cursor {
                break;
            }
            let body = &source[cursor + 1..closing];
            let source_line = source[..name_start].matches('\n').count() as u32 + 1;
            let origin = parsed_origin
                .get_or_insert_with(|| provenance::ParsedOrigin::new(source, dialect))
                .view();
            let environment = query_output::Environment {
                origin,
                ..environment
            };
            let shape = if recursive || recursive_ctes::implicit(dialect) {
                recursive_ctes::infer(
                    &recursive_ctes::Definition {
                        name: &name,
                        quoted: matches!(source.as_bytes()[name_start], b'"' | b'`' | b'['),
                        names: &explicit_columns,
                        body,
                        upper: &upper[cursor + 1..closing],
                        line: source_line,
                    },
                    &working_schema,
                    uri,
                    dialect,
                    environment,
                )
            } else {
                None
            };
            if inside_body
                && shape
                    .as_ref()
                    .is_none_or(|shape| position - cursor - 1 < shape.member_start)
            {
                break;
            }
            let columns = shape.map(|shape| shape.columns).unwrap_or_else(|| {
                query_output::infer_with_environment(
                    body,
                    &working_schema,
                    uri,
                    source_line,
                    dialect,
                    &explicit_columns,
                    environment,
                )
            });
            let table =
                local_relation_table(name, "COMMON TABLE EXPRESSION", columns, uri, source_line);
            install_local_table(&mut working_schema, table.clone());
            tables.push(table);
            if inside_body || closing == source.len() {
                break;
            }
            cursor = skip_sql_trivia(source, closing + 1);
            if !source[cursor..].starts_with(',') {
                break;
            }
            cursor = skip_sql_trivia(source, cursor + 1);
        }
        search = with_start + "WITH".len();
    }
    tables
}

fn derived_tables_before_cursor(
    source: &str,
    upper: &str,
    schema: &Schema,
    uri: &str,
    position: usize,
    dialect: Option<&str>,
    environment: query_output::Environment<'_>,
) -> Vec<Table> {
    let mut parsed_origin = None;
    derived_tables_with_inference(
        source,
        upper,
        schema,
        uri,
        position,
        |body, schema, line, names| {
            let origin = parsed_origin
                .get_or_insert_with(|| provenance::ParsedOrigin::new(source, dialect))
                .view();
            query_output::infer_with_environment(
                body,
                schema,
                uri,
                line,
                dialect,
                names,
                query_output::Environment {
                    origin,
                    ..environment
                },
            )
        },
    )
}

fn derived_tables_with_inference(
    source: &str,
    upper: &str,
    schema: &Schema,
    uri: &str,
    position: usize,
    mut infer: impl FnMut(&str, &Schema, u32, &[String]) -> Vec<Column>,
) -> Vec<Table> {
    let mut tables = Vec::new();
    let mut working_schema = None;
    for source_start in local_relation_source_starts(upper) {
        if !scope_contains(upper, source_start, position) {
            continue;
        }
        let mut opening = skip_local_whitespace(source, source_start);
        let after_lateral = skip_optional_keyword_sequence(source, opening, &["LATERAL"]);
        if after_lateral != opening {
            opening = after_lateral;
        }
        if !source[opening..].starts_with('(') {
            continue;
        }
        let Some(closing) = matching_parenthesis_in_searchable(upper, opening) else {
            continue;
        };
        if position > opening && position <= closing {
            continue;
        }
        let mut alias_start = skip_local_whitespace(source, closing + 1);
        if upper[alias_start..].starts_with("AS")
            && local_keyword_boundary(upper, alias_start, "AS")
        {
            alias_start = skip_local_whitespace(source, alias_start + "AS".len());
        }
        let Some((alias, alias_offset, alias_end)) =
            read_local_identifier_after(source, alias_start)
        else {
            continue;
        };
        if !matches!(
            source.as_bytes().get(alias_offset),
            Some(b'"' | b'`' | b'[')
        ) && Keywords::is_keyword(&alias)
        {
            continue;
        }
        let body = &source[opening + 1..closing];
        if !upper[opening + 1..closing].contains("SELECT") {
            continue;
        }
        let source_line = source[..alias_offset].matches('\n').count() as u32 + 1;
        let mut columns = infer(
            body,
            working_schema.as_ref().unwrap_or(schema),
            source_line,
            &correlation_column_names_after(source, upper, alias_end).unwrap_or_default(),
        );
        if derived_is_null_extended(&upper[..opening]) {
            for column in &mut columns {
                column.nullable = true;
            }
        }
        let table = local_relation_table(alias, "DERIVED TABLE", columns, uri, source_line);
        install_local_table(
            working_schema.get_or_insert_with(|| schema.clone()),
            table.clone(),
        );
        tables.push(table);
    }
    tables
}

fn derived_is_null_extended(prefix: &str) -> bool {
    let words: Vec<_> = prefix
        .trim_end()
        .rsplit(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|word| !word.is_empty())
        .take(4)
        .collect();
    matches!(words.first().copied(), Some("APPLY")) && words.get(1) == Some(&"OUTER")
        || matches!(words.first().copied(), Some("JOIN" | "LATERAL"))
            && words.iter().any(|word| matches!(*word, "LEFT" | "FULL"))
}

pub(super) fn correlated_tables_before_cursor(
    source: &str,
    upper: &str,
    schema: &Schema,
    uri: &str,
    position: usize,
) -> Vec<Table> {
    let mut tables = Vec::new();
    for source_start in local_relation_source_starts(upper) {
        if !scope_contains(upper, source_start, position) {
            continue;
        }
        let mut cursor = skip_local_whitespace(source, source_start);
        for modifier in ["LATERAL", "ONLY"] {
            let advanced = skip_optional_keyword_sequence(source, cursor, &[modifier]);
            if advanced != cursor {
                cursor = advanced;
            }
        }
        let Some((reference, reference_start, reference_end)) =
            read_local_identifier_path_after(source, cursor)
        else {
            continue;
        };
        cursor = skip_local_whitespace(source, reference_end);
        let is_table_function = source[cursor..].starts_with('(');
        if is_table_function {
            let Some(closing) = matching_parenthesis_in_searchable(upper, cursor) else {
                continue;
            };
            cursor = skip_local_whitespace(source, closing + 1);
            let after_ordinality =
                skip_optional_keyword_sequence(source, cursor, &["WITH", "ORDINALITY"]);
            if after_ordinality != cursor {
                cursor = after_ordinality;
            }
        }
        let after_as = skip_optional_keyword_sequence(source, cursor, &["AS"]);
        if after_as != cursor {
            cursor = after_as;
        }
        let Some((alias, alias_start, alias_end)) = read_local_identifier_after(source, cursor)
        else {
            continue;
        };
        if !matches!(source.as_bytes().get(alias_start), Some(b'"' | b'`' | b'['))
            && Keywords::is_keyword(&alias)
        {
            continue;
        }
        cursor = skip_local_whitespace(source, alias_end);
        // SQL Server table hints are not PostgreSQL correlation columns. WITH
        // makes the distinction unambiguous; the legacy `(NOLOCK)` form is
        // filtered by the known hint vocabulary below.
        if upper[cursor..].starts_with("WITH") && local_keyword_boundary(upper, cursor, "WITH") {
            continue;
        }
        let Some(column_aliases) = correlation_column_names_after(source, upper, cursor) else {
            continue;
        };
        if column_aliases.is_empty() || is_sqlserver_table_hint_list(&column_aliases) {
            continue;
        }

        let table_name = if is_table_function {
            SqlParser::identifier_last_part(&reference)
        } else {
            alias.clone()
        };
        let base_columns = schema
            .tables
            .iter()
            .find(|table| schema_table_matches(schema, &reference, table))
            .map(|table| table.columns.clone())
            .unwrap_or_default();
        let source_line = source[..alias_start.min(reference_start)]
            .matches('\n')
            .count() as u32
            + 1;
        let columns = merge_local_column_aliases(base_columns, column_aliases, uri, source_line);
        if is_table_function && alias != table_name {
            tables.push(local_relation_table(
                alias,
                "TABLE FUNCTION",
                columns.clone(),
                uri,
                source_line,
            ));
        }
        tables.push(local_relation_table(
            table_name,
            if is_table_function {
                "TABLE FUNCTION"
            } else {
                "CORRELATED TABLE"
            },
            columns,
            uri,
            source_line,
        ));
    }
    tables
}

pub(super) fn merge_local_column_aliases(
    mut columns: Vec<Column>,
    aliases: Vec<String>,
    uri: &str,
    line: u32,
) -> Vec<Column> {
    if columns.is_empty() {
        return aliases
            .into_iter()
            .map(|name| local_relation_column(name, "unknown", uri, line))
            .collect();
    }
    for (column, alias) in columns.iter_mut().zip(aliases) {
        column.name = alias;
        column.source_location = Some((uri.to_string(), line));
    }
    columns
}

pub(super) fn correlation_column_names_after(
    source: &str,
    upper: &str,
    start: usize,
) -> Option<Vec<String>> {
    let opening = skip_local_whitespace(source, start);
    if !source[opening..].starts_with('(') {
        return None;
    }
    let closing = matching_parenthesis_in_searchable(upper, opening)?;
    let mut names = Vec::new();
    for (item_start, item_end) in split_top_level_ranges_in_searchable(upper, opening + 1, closing)
    {
        let (name, _, name_end) = read_local_identifier_after(source, item_start)?;
        if !upper[name_end..item_end].trim().is_empty() {
            return None;
        }
        names.push(name);
    }
    Some(names)
}

pub(super) fn is_sqlserver_table_hint_list(names: &[String]) -> bool {
    const TABLE_HINTS: &[&str] = &[
        "FORCESEEK",
        "FORCESCAN",
        "HOLDLOCK",
        "INDEX",
        "NOEXPAND",
        "NOLOCK",
        "NOWAIT",
        "PAGLOCK",
        "READCOMMITTED",
        "READCOMMITTEDLOCK",
        "READPAST",
        "READUNCOMMITTED",
        "REPEATABLEREAD",
        "ROWLOCK",
        "SERIALIZABLE",
        "SNAPSHOT",
        "SPATIAL_WINDOW_MAX_CELLS",
        "TABLOCK",
        "TABLOCKX",
        "UPDLOCK",
        "XLOCK",
    ];
    names.iter().any(|name| {
        TABLE_HINTS
            .iter()
            .any(|hint| name.eq_ignore_ascii_case(hint))
    })
}

pub(super) fn local_relation_table(
    name: String,
    object_type: &str,
    columns: Vec<Column>,
    uri: &str,
    line: u32,
) -> Table {
    Table {
        name,
        object_type: Some(object_type.to_string()),
        columns,
        indexes: Vec::new(),
        constraints: Vec::new(),
        comment: Some(format!("{object_type} visible in the current console")),
        source_location: Some((uri.to_string(), line)),
    }
}

pub(super) fn local_relation_column(name: String, data_type: &str, uri: &str, line: u32) -> Column {
    Column {
        name,
        data_type: data_type.to_string(),
        nullable: true,
        primary_key: false,
        unique: false,
        indexed: false,
        default_value: None,
        auto_increment: false,
        generated: false,
        hidden: false,
        comment: None,
        source_location: Some((uri.to_string(), line)),
    }
}

fn install_local_table(schema: &mut Schema, table: Table) {
    crate::relation_bindings::install(schema, table);
}

// A relation in a closed sibling subquery is invisible to this cursor.
fn scope_contains(searchable: &str, offset: usize, cursor: usize) -> bool {
    SqlParser::query_source_visible(searchable, offset, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{position::lsp_position_at_end, schema::SchemaId};

    fn at_marker(sql: &str) -> Schema {
        let offset = sql.find('|').unwrap();
        let position = lsp_position_at_end(&sql[..offset]);
        let sql = sql.replacen('|', "", 1);
        augment_schema_with_local_relations(
            Schema {
                id: SchemaId::new(),
                server: None,
                namespace: None,
                catalog: None,
                database: "public".into(),
                server_version: None,
                functions: vec![],
                source_uri: None,
                tables: vec![Table {
                    name: "users".into(),
                    columns: vec![
                        Column {
                            name: "id".into(),
                            data_type: "integer".into(),
                            ..Column::default()
                        },
                        Column {
                            name: "name".into(),
                            data_type: "text".into(),
                            ..Column::default()
                        },
                    ],
                    ..Table::default()
                }],
            },
            &sql,
            position,
            "oxide://query/scope.sql",
            Some("postgres"),
        )
    }
    fn names(schema: &Schema, name: &str) -> Vec<String> {
        schema
            .tables
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("missing {name}"))
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect()
    }
    #[test]
    fn derived_projection_after_cursor_is_visible() {
        let schema = at_marker("SELECT sq.| FROM (SELECT id, name AS display_name FROM users) sq");
        assert_eq!(names(&schema, "sq"), ["id", "display_name"]);
    }

    #[test]
    fn temporary_query_semicolons_in_literals_do_not_truncate_columns() {
        let schema = at_marker(
            "CREATE TEMP TABLE draft AS SELECT ';' AS marker, 1 AS id; SELECT draft.| FROM draft",
        );
        assert_eq!(names(&schema, "draft"), vec!["marker", "id"]);
    }
    #[test]
    fn materialized_cte_chain_inherits_star_columns() {
        for hint in [
            "MATERIALIZED",
            "NOT MATERIALIZED",
            "/* comment */ MATERIALIZED",
        ] {
            let schema = at_marker(&format!("WITH a AS {hint} (SELECT id, name FROM users), b AS (SELECT * FROM a) SELECT b.| FROM b"));
            assert_eq!(names(&schema, "b"), ["id", "name"]);
        }
    }
    #[test]
    fn ctes_do_not_leak_across_statements_or_closed_subqueries() {
        for sql in [
            "WITH ghost AS (SELECT id FROM users) SELECT * FROM ghost; SELECT | FROM users",
            "SELECT (WITH ghost AS (SELECT id FROM users) SELECT * FROM ghost), | FROM users",
        ] {
            assert!(!at_marker(sql).tables.iter().any(|t| t.name == "ghost"));
        }
    }
    #[test]
    fn nested_cte_shadows_outer_only_inside_its_scope() {
        let inner = at_marker("WITH a(x) AS (SELECT id FROM users) SELECT (WITH a(y) AS (SELECT name FROM users) SELECT a.| FROM a) FROM a");
        assert_eq!(names(&inner, "a"), ["y"]);
        let outer = at_marker("WITH a(x) AS (SELECT id FROM users) SELECT (WITH a(y) AS (SELECT name FROM users) SELECT * FROM a), a.| FROM a");
        assert_eq!(names(&outer, "a"), ["x"]);
    }
    #[test]
    fn recursive_columns_are_visible_in_unfinished_body() {
        let schema = at_marker(
            "WITH RECURSIVE chain(id) AS (SELECT id FROM users UNION ALL SELECT chain.| FROM chain",
        );
        assert_eq!(names(&schema, "chain"), ["id"]);
        let plain = at_marker("WITH chain(id) AS (SELECT chain.| FROM users)");
        assert!(!plain.tables.iter().any(|t| t.name == "chain"));
    }
    #[test]
    fn later_cte_is_not_visible_inside_earlier_body() {
        let schema = at_marker(
            "WITH a AS (SELECT | FROM users), later(x) AS (SELECT id FROM users) SELECT * FROM a",
        );
        assert!(!schema.tables.iter().any(|t| t.name == "later"));
    }
    #[test]
    fn derived_from_cte_and_navigation_preserve_document_line() {
        let schema = at_marker(
            "-- 😀\nWITH a AS (SELECT id FROM users)\nSELECT sq.| FROM (SELECT * FROM a) sq",
        );
        assert_eq!(names(&schema, "sq"), ["id"]);
        assert_eq!(
            schema
                .tables
                .iter()
                .find(|t| t.name == "sq")
                .unwrap()
                .source_location,
            Some(("oxide://query/scope.sql".into(), 3))
        );
    }
    #[test]
    fn quoted_cte_and_qualified_star_keep_only_the_selected_source() {
        let schema = at_marker("WITH \"a\"(left_id) AS (SELECT 1), b(right_id) AS (SELECT 2), c AS (SELECT x.* FROM \"a\" x JOIN b ON true) SELECT c.| FROM c");
        assert_eq!(names(&schema, "c"), ["left_id"]);
    }

    #[test]
    fn following_query_without_semicolon_does_not_replace_derived_shape() {
        let schema = at_marker("SELECT sq.| FROM (SELECT 1 AS current_id) sq\n\nSELECT * FROM (SELECT 2 AS next_id) sq\n\nSELECT * FROM (SELECT 3 AS last_id) sq");
        assert_eq!(names(&schema, "sq"), ["current_id"]);
    }

    #[test]
    fn union_projection_uses_first_branch_names() {
        let schema =
            at_marker("WITH a AS (SELECT 1 AS seed UNION ALL SELECT 2 AS other) SELECT a.| FROM a");
        assert_eq!(names(&schema, "a"), ["seed"]);
    }
}
