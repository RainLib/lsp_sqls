//! Diagnostics for the SQL language server.
mod ambiguity;
mod inspection_context;
mod lateral_views;
mod qualify;
mod statement_structure;
use crate::dialect::Dialect;
use crate::parser::SqlParser;
use crate::schema::Schema;
pub(super) use inspection_context::Context as InspectionContext;
#[cfg(test)]
pub(super) use statement_structure::FALLBACK_PARSE_COUNT;
use tower_lsp::lsp_types::*;
#[cfg(test)]
pub(super) async fn document_diagnostics(
    dialect: &dyn Dialect,
    text: &str,
    schema: Option<&Schema>,
) -> Vec<Diagnostic> {
    document_diagnostics_with_metadata(dialect, text, schema, None).await
}
pub(super) async fn document_diagnostics_with_metadata(
    dialect: &dyn Dialect,
    text: &str,
    schema: Option<&Schema>,
    manager: Option<&crate::schema::SchemaManager>,
) -> Vec<Diagnostic> {
    if !matches!(
        dialect.name(),
        "postgres" | "mysql" | "clickhouse" | "hive" | "sqlserver" | "sqlite" | "duckdb"
    ) {
        let mut diagnostics = dialect.parse(text, schema).await;
        diagnostics.extend(qualify::diagnostics(text, dialect.name(), None));
        diagnostics.extend(lateral_views::diagnostics(text, dialect.name(), None));
        return diagnostics;
    }
    let parsed = dialect.parse_with_syntax(text, schema).await;
    // A snapshot is request-owned, and must describe the exact original text.
    // Fall back to the established validation API for a mismatched provider.
    let (mut diagnostics, tree) = if parsed.source == text {
        (parsed.diagnostics, parsed.tree)
    } else {
        (dialect.parse(text, schema).await, None)
    };
    diagnostics.extend(qualify::diagnostics(text, dialect.name(), tree.as_ref()));
    diagnostics.extend(lateral_views::diagnostics(
        text,
        dialect.name(),
        tree.as_ref(),
    ));
    diagnostics.extend(sql_inspection_diagnostics_with_metadata(
        text,
        schema,
        dialect.name(),
        tree.as_ref(),
        manager,
    ));
    diagnostics
}

#[cfg(test)]
pub(super) fn sql_inspection_diagnostics(
    text: &str,
    schema: Option<&Schema>,
    dialect_name: &str,
) -> Vec<Diagnostic> {
    sql_inspection_diagnostics_with_syntax(text, schema, dialect_name, None)
}
#[cfg(test)]
fn sql_inspection_diagnostics_with_syntax(
    text: &str,
    schema: Option<&Schema>,
    dialect_name: &str,
    tree: Option<&tree_sitter::Tree>,
) -> Vec<Diagnostic> {
    sql_inspection_diagnostics_with_metadata(text, schema, dialect_name, tree, None)
}
fn sql_inspection_diagnostics_with_metadata(
    text: &str,
    schema: Option<&Schema>,
    dialect_name: &str,
    tree: Option<&tree_sitter::Tree>,
    manager: Option<&crate::schema::SchemaManager>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let context = InspectionContext::with_syntax(text, dialect_name, tree);
    for span in context.statements() {
        let statement_start = span.start;
        let normalized = &context.normalized[span.clone()];
        let suppressed = context.suppressions(&span);
        for mutation in context
            .mutations(&span)
            .filter(|m| context.suppressions_at(&span, m.keyword) & 1 == 0)
        {
            let warning_span = if mutation.root
                && mutation.owner.start >= span.start
                && mutation.owner.end <= span.end
            {
                span.clone()
            } else {
                mutation.range.clone()
            };
            diagnostics.push(Diagnostic {
                // Most statements produce no diagnostic. Computing a range
                // here avoids rescanning every full-source prefix eagerly.
                range: context.range(warning_span.start, warning_span.end),
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

        for join in context
            .joins(&span)
            .filter(|j| context.suppressions_at(&span, j.keyword) & 2 == 0)
        {
            diagnostics.push(Diagnostic {
                range: context.range(join.keyword, join.keyword + 4),
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String("OXIDE002".into())),
                source: Some("oxide-inspections".into()),
                message: "JOIN has no ON or USING condition and may create a Cartesian product."
                    .into(),
                ..Default::default()
            });
        }

        if let Some(star_relative) = select_star_offset(normalized) {
            if suppressed & 4 == 0 {
                diagnostics.push(Diagnostic {
                    range: context.range(
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

        if suppressed & 16 == 0 {
            diagnostics.extend(dialect_risk_diagnostics(
                &context,
                normalized,
                statement_start,
            ));
        }
    }
    diagnostics.extend(ambiguity::diagnostics_with_metadata(
        &context, schema, manager,
    ));
    diagnostics
}

fn dialect_risk_diagnostics(
    context: &InspectionContext,
    normalized: &str,
    statement_start: usize,
) -> Vec<Diagnostic> {
    if crate::placeholder::SqlPlaceholderDialect::for_name(Some(context.dialect))
        == crate::placeholder::SqlPlaceholderDialect::Postgres
        || context.dialect.eq_ignore_ascii_case("clickhouse")
    {
        return Vec::new();
    }
    let mut risks = Vec::new();
    for (needle, message) in [
        ("ILIKE", "ILIKE may not be portable to this dialect."),
        (
            "::",
            "The PostgreSQL-style :: cast may not be portable; use CAST(value AS type).",
        ),
    ] {
        for (start, matched) in normalized.match_indices(needle) {
            if needle == "ILIKE" && !keyword_at(normalized, start, needle) {
                continue;
            }
            if needle == "::"
                && crate::sql_lexical::lexical_rules_for(context.dialect).placeholder
                    == crate::placeholder::SqlPlaceholderDialect::SqlServer
                && sqlserver_static_call(
                    &context.text[statement_start..statement_start + normalized.len()],
                    normalized,
                    start,
                )
            {
                continue;
            }
            risks.push(Diagnostic {
                range: context.range(
                    statement_start + start,
                    statement_start + start + matched.len(),
                ),
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String("OXIDE005".into())),
                source: Some("oxide-inspections".into()),
                message: message.into(),
                ..Default::default()
            });
        }
    }
    risks
}
fn sqlserver_static_call(source: &str, normalized: &str, at: usize) -> bool {
    let before = normalized[..at].trim_end();
    let word = before
        .rsplit(|ch: char| !SqlParser::is_identifier_char(ch))
        .next()
        .unwrap_or_default();
    let named = word
        .chars()
        .next()
        .is_some_and(|ch| ch.is_alphabetic() || ch == '_')
        || source[..at].trim_end().ends_with([']', '"']);
    let tail = normalized[at + 2..].trim_start();
    let method = tail
        .chars()
        .take_while(|ch| SqlParser::is_identifier_char(*ch))
        .map(char::len_utf8)
        .sum::<usize>();
    named && tail[method..].trim_start().starts_with('(')
}
fn keyword_at(source: &str, start: usize, keyword: &str) -> bool {
    let end = start + keyword.len();
    !source[..start]
        .chars()
        .next_back()
        .is_some_and(SqlParser::is_identifier_char)
        && !source[end..]
            .chars()
            .next()
            .is_some_and(SqlParser::is_identifier_char)
}

pub(super) fn contains_sql_keyword(normalized: &str, keyword: &str) -> bool {
    normalized
        .match_indices(keyword)
        .any(|(start, _)| keyword_at(normalized, start, keyword))
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
