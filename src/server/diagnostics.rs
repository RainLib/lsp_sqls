//! Diagnostics for the SQL language server.
use super::document_structure::range_for_offsets;
use super::schema_resolution::schema_table_matches;
use super::semantic_tokens::sql_identifier_tokens;
use crate::dialect::Dialect;
use crate::parser::SqlParser;
use crate::schema::Schema;
use std::collections::HashMap;
use std::collections::HashSet;
use tower_lsp::lsp_types::*;
pub(super) async fn document_diagnostics(
    dialect: &dyn Dialect,
    text: &str,
    schema: Option<&Schema>,
) -> Vec<Diagnostic> {
    let mut diagnostics = dialect.parse(text, schema).await;
    if matches!(dialect.name(), "postgres" | "mysql" | "clickhouse" | "hive") {
        diagnostics.extend(sql_inspection_diagnostics(text, schema, dialect.name()));
    }
    diagnostics
}

pub(super) fn sql_inspection_diagnostics(
    text: &str,
    schema: Option<&Schema>,
    dialect_name: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut offset = 0usize;
    for segment in text.split_inclusive(';') {
        let trimmed_start = segment.len() - segment.trim_start().len();
        let trimmed_end = segment.trim_end().len();
        if trimmed_start >= trimmed_end {
            offset += segment.len();
            continue;
        }
        let statement_start = offset + trimmed_start;
        let statement_end = offset + trimmed_end;
        let statement = &text[statement_start..statement_end];
        let normalized = SqlParser::mask_sql_noise(statement).to_ascii_uppercase();
        let statement_range = range_for_offsets(text, statement_start, statement_end);

        let mutating_without_where = (normalized.trim_start().starts_with("UPDATE ")
            || normalized.trim_start().starts_with("DELETE "))
            && !contains_sql_keyword(&normalized, "WHERE");
        if mutating_without_where && !inspection_suppressed(statement, "OXIDE001") {
            diagnostics.push(Diagnostic {
                range: statement_range,
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String("OXIDE001".to_string())),
                code_description: None,
                source: Some("oxide-inspections".to_string()),
                message: "Mutation has no WHERE clause and may affect every matching row."
                    .to_string(),
                related_information: None,
                tags: None,
                data: None,
            });
        }

        if let Some(join_relative) = join_without_condition_offset(&normalized) {
            if !inspection_suppressed(statement, "OXIDE002") {
                diagnostics.push(Diagnostic {
                    range: range_for_offsets(
                        text,
                        statement_start + join_relative,
                        statement_start + join_relative + "JOIN".len(),
                    ),
                    severity: Some(DiagnosticSeverity::WARNING),
                    code: Some(NumberOrString::String("OXIDE002".to_string())),
                    code_description: None,
                    source: Some("oxide-inspections".to_string()),
                    message:
                        "JOIN has no ON or USING condition and may create a Cartesian product."
                            .to_string(),
                    related_information: None,
                    tags: None,
                    data: None,
                });
            }
        }

        if let Some(star_relative) = select_star_offset(&normalized) {
            if !inspection_suppressed(statement, "OXIDE003") {
                diagnostics.push(Diagnostic {
                    range: range_for_offsets(
                        text,
                        statement_start + star_relative,
                        statement_start + star_relative + 1,
                    ),
                    severity: Some(DiagnosticSeverity::HINT),
                    code: Some(NumberOrString::String("OXIDE003".to_string())),
                    code_description: None,
                    source: Some("oxide-inspections".to_string()),
                    message: "Explicit columns are safer when a schema changes.".to_string(),
                    related_information: None,
                    tags: None,
                    data: None,
                });
            }
        }

        if !inspection_suppressed(statement, "OXIDE004") {
            diagnostics.extend(ambiguous_column_diagnostics(
                text,
                statement,
                statement_start,
                schema,
            ));
        }

        if !inspection_suppressed(statement, "OXIDE005") {
            diagnostics.extend(dialect_risk_diagnostics(
                text,
                statement,
                statement_start,
                dialect_name,
            ));
        }

        offset += segment.len();
    }
    diagnostics
}

pub(super) fn inspection_suppressed(statement: &str, code: &str) -> bool {
    statement
        .to_ascii_uppercase()
        .contains(&format!("NOINSPECTION {}", code.to_ascii_uppercase()))
}

pub(super) fn join_without_condition_span(normalized: &str) -> Option<(usize, usize)> {
    for (offset, _) in normalized.match_indices("JOIN") {
        let before = normalized[..offset].trim_end();
        let keyword_before = before.split_whitespace().next_back().unwrap_or_default();
        if matches!(keyword_before, "CROSS" | "NATURAL") {
            continue;
        }
        let tail = &normalized[offset + "JOIN".len()..];
        let boundary = [
            " JOIN ", " WHERE ", " GROUP ", " HAVING ", " ORDER ", " LIMIT ", " UNION ",
        ]
        .iter()
        .filter_map(|keyword| tail.find(keyword))
        .min()
        .unwrap_or(tail.len());
        let join_clause = &tail[..boundary];
        if !contains_sql_keyword(join_clause, "ON") && !contains_sql_keyword(join_clause, "USING") {
            let relation_end = join_clause.trim_end_matches(';').trim_end().len();
            return Some((offset, offset + "JOIN".len() + relation_end));
        }
    }
    None
}

pub(super) fn join_without_condition_offset(normalized: &str) -> Option<usize> {
    join_without_condition_span(normalized).map(|(offset, _)| offset)
}

pub(super) fn ambiguous_column_diagnostics(
    full_text: &str,
    statement: &str,
    statement_start: usize,
    schema: Option<&Schema>,
) -> Vec<Diagnostic> {
    let Some(schema) = schema else {
        return Vec::new();
    };
    let mut parser = SqlParser::new();
    let Some(tree) = parser.parse(statement).tree else {
        return Vec::new();
    };
    let referenced = parser.extract_tables(&tree, statement);
    if referenced.len() < 2 {
        return Vec::new();
    }
    let referenced_tables = schema
        .tables
        .iter()
        .filter(|table| {
            referenced
                .iter()
                .any(|reference| schema_table_matches(schema, reference, table))
        })
        .collect::<Vec<_>>();
    if referenced_tables.len() < 2 {
        return Vec::new();
    }
    let mut counts = HashMap::<String, usize>::new();
    for table in referenced_tables {
        for column in &table.columns {
            *counts.entry(column.name.to_ascii_lowercase()).or_default() += 1;
        }
    }
    let ambiguous = counts
        .into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name))
        .collect::<HashSet<_>>();
    if ambiguous.is_empty() {
        return Vec::new();
    }

    sql_identifier_tokens(statement)
        .into_iter()
        .filter_map(|(identifier, start, end)| {
            if !ambiguous.contains(&identifier.to_ascii_lowercase()) {
                return None;
            }
            let qualified = statement[..start]
                .chars()
                .next_back()
                .is_some_and(|character| character == '.');
            if qualified {
                return None;
            }
            Some(Diagnostic {
                range: range_for_offsets(
                    full_text,
                    statement_start + start,
                    statement_start + end,
                ),
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String("OXIDE004".to_string())),
                code_description: None,
                source: Some("oxide-inspections".to_string()),
                message: format!(
                    "Column `{identifier}` exists in multiple referenced relations; qualify it with an alias."
                ),
                related_information: None,
                tags: None,
                data: None,
            })
        })
        .collect()
}

pub(super) fn dialect_risk_diagnostics(
    full_text: &str,
    statement: &str,
    statement_start: usize,
    dialect_name: &str,
) -> Vec<Diagnostic> {
    if dialect_name == "postgres" {
        return Vec::new();
    }
    let normalized = SqlParser::mask_sql_noise(statement).to_ascii_uppercase();
    let mut risks = Vec::new();
    for (needle, message) in [
        (
            "ILIKE",
            "ILIKE is PostgreSQL-specific and is not portable to this dialect.",
        ),
        (
            "::",
            "The :: cast syntax is PostgreSQL-specific; use CAST(value AS type).",
        ),
    ] {
        for (start, matched) in normalized.match_indices(needle) {
            risks.push(Diagnostic {
                range: range_for_offsets(
                    full_text,
                    statement_start + start,
                    statement_start + start + matched.len(),
                ),
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String("OXIDE005".to_string())),
                code_description: None,
                source: Some("oxide-inspections".to_string()),
                message: message.to_string(),
                related_information: None,
                tags: None,
                data: None,
            });
        }
    }
    risks
}

pub(super) fn contains_sql_keyword(normalized: &str, keyword: &str) -> bool {
    normalized.match_indices(keyword).any(|(start, matched)| {
        let end = start + matched.len();
        let before = normalized[..start].chars().next_back();
        let after = normalized[end..].chars().next();
        !before.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
            && !after.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
    })
}

pub(super) fn select_star_offset(normalized: &str) -> Option<usize> {
    let select = normalized.find("SELECT")?;
    let mut offset = select + "SELECT".len();
    while normalized[offset..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
    {
        offset += normalized[offset..].chars().next()?.len_utf8();
    }
    (normalized[offset..].starts_with('*')).then_some(offset)
}
