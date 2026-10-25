//! Explicit T-SQL TABLE declarations in their existing batch/module scope.
use super::{local_relation_table, parse_temporary_column_with_mask, syntax::*};
use crate::{parser::SqlParser, placeholder::SqlPlaceholderDialect, schema::Table};
pub(super) fn tables(text: &str, cursor: usize, uri: &str, dialect: Option<&str>) -> Vec<Table> {
    if SqlPlaceholderDialect::for_name(dialect) != SqlPlaceholderDialect::SqlServer
        || !text.as_bytes().contains(&b'@')
    {
        return Vec::new();
    }
    let noise = SqlParser::mask_sql_noise_for_dialect(text, "sqlserver");
    let content = SqlParser::mask_sql_operand_context_for_dialect(text, "sqlserver");
    let declared = crate::server::tsql_variables::declared(text, &noise, &content, cursor);
    let mut tables = Vec::new();
    for variable in declared
        .variables
        .into_iter()
        .filter(|variable| !variable.scalar && variable.visible_from <= cursor)
    {
        let Some((_, length)) = crate::server::routine_identity::read_parameter_name(
            &text[variable.name_start..],
            "sqlserver",
        ) else {
            continue;
        };
        let mut start = skip_local_whitespace(&content, variable.name_start + length);
        if crate::server::execute_context::keyword_at(&noise, start, "AS") {
            start = skip_local_whitespace(&content, start + 2);
        }
        if !crate::server::execute_context::keyword_at(&noise, start, "TABLE") {
            continue;
        }
        let open = skip_local_whitespace(&content, start + 5);
        let Some(close) = matching_parenthesis_in_searchable(&noise, open) else {
            continue;
        };
        if close >= variable.visible_from {
            continue;
        }
        let mut line_offset = 0;
        let mut line = 1;
        let columns = split_top_level_ranges_in_searchable(&noise, open + 1, close)
            .into_iter()
            .filter_map(|(start, end)| {
                let definition = &text[start..end];
                line += text[line_offset..start]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count() as u32;
                line_offset = start;
                let mut column =
                    parse_temporary_column_with_mask(definition, &noise[start..end], uri, line)?;
                let (_, _, name_end) = read_local_identifier_after(definition, 0)?;
                let type_start = skip_local_whitespace(definition, name_end);
                let (_, _, word_end) = read_local_identifier_after(definition, type_start)?;
                let after = skip_local_whitespace(definition, word_end);
                let type_end = if definition[after..].starts_with('(') {
                    matching_parenthesis_in_searchable(&noise[start..end], after)? + 1
                } else {
                    word_end
                };
                column.data_type =
                    crate::server::tsql_types::scalar_type(&definition[type_start..type_end])
                        .unwrap_or_else(|| "unknown".into());
                // T-SQL PRIMARY KEY columns are non-nullable even when NULL
                // was omitted. Do not impose this on SQLite's legacy PK rules.
                column.nullable &= !column.primary_key;
                Some(column)
            })
            .collect();
        tables.push(local_relation_table(
            variable.name,
            "TABLE VARIABLE",
            columns,
            uri,
            text[..variable.name_start]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count() as u32
                + 1,
        ));
    }
    tables
}
