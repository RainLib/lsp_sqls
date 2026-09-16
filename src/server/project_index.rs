//! Project index for the SQL language server.
use super::document_structure::range_for_offsets;
use super::limits::{PROJECT_SQL_INDEX_MAX_BYTES, PROJECT_SQL_INDEX_MAX_OCCURRENCES};
use super::local_relations::{
    local_keyword_boundary, local_relation_source_starts, read_local_identifier_after,
    skip_local_whitespace,
};
use super::project_types::{
    ProjectSqlSymbolKind, ProjectSqlSymbolOccurrence, ProjectSqlSymbolRole,
};
use crate::parser::SqlParser;
use crate::schema::SchemaId;
use crate::token::Keywords;
use tower_lsp::lsp_types::*;
pub(super) fn project_uri_scope(uri: &str) -> Option<&str> {
    let prefix = "oxide://project/";
    let rest = uri.strip_prefix(prefix)?;
    let project_end = rest.find('/').unwrap_or(rest.len());
    Some(&rest[..project_end])
}

pub(super) fn documents_share_project_sql_scope(
    origin_uri: &str,
    candidate_uri: &str,
    origin_schema_id: Option<SchemaId>,
    candidate_schema_id: Option<SchemaId>,
) -> bool {
    match (origin_schema_id, candidate_schema_id) {
        (Some(origin), Some(candidate)) => origin == candidate,
        (Some(_), None) | (None, Some(_)) => false,
        (None, None) => {
            origin_uri == candidate_uri
                || project_uri_scope(origin_uri)
                    .zip(project_uri_scope(candidate_uri))
                    .is_some_and(|(origin, candidate)| origin == candidate)
        }
    }
}

pub(super) fn project_sql_symbols_match(
    left: &ProjectSqlSymbolOccurrence,
    right: &ProjectSqlSymbolOccurrence,
) -> bool {
    if left.kind.is_routine() != right.kind.is_routine() {
        return false;
    }
    if left.kind.is_routine()
        && left.kind != right.kind
        && left.role == ProjectSqlSymbolRole::Definition
        && right.role == ProjectSqlSymbolRole::Definition
    {
        return false;
    }
    let left_parts = left.normalized_name.split('.').collect::<Vec<_>>();
    let right_parts = right.normalized_name.split('.').collect::<Vec<_>>();
    let shared = left_parts.len().min(right_parts.len());
    shared > 0
        && left_parts[left_parts.len() - shared..] == right_parts[right_parts.len() - shared..]
}

pub(super) fn project_sql_symbol_container(uri: &Url) -> String {
    if uri.scheme() == "oxide" && uri.host_str() == Some("project") {
        return uri
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|path| !path.is_empty())
            .unwrap_or("Project SQL")
            .to_string();
    }
    uri.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|path| !path.is_empty())
        .unwrap_or("SQL document")
        .to_string()
}

pub(super) fn project_sql_symbol_occurrences(source: &str) -> Vec<ProjectSqlSymbolOccurrence> {
    if source.len() > PROJECT_SQL_INDEX_MAX_BYTES {
        return Vec::new();
    }
    let searchable = SqlParser::mask_sql_noise(source);
    let upper = searchable.to_ascii_uppercase();
    let cte_scopes = project_cte_scopes(source, &searchable);
    let mut occurrences = Vec::new();

    let definitions = [
        ("CREATE TABLE", ProjectSqlSymbolKind::Table),
        ("CREATE OR REPLACE TABLE", ProjectSqlSymbolKind::Table),
        ("CREATE TEMP TABLE", ProjectSqlSymbolKind::Table),
        ("CREATE TEMPORARY TABLE", ProjectSqlSymbolKind::Table),
        ("CREATE GLOBAL TEMPORARY TABLE", ProjectSqlSymbolKind::Table),
        ("CREATE FOREIGN TABLE", ProjectSqlSymbolKind::Table),
        ("CREATE EXTERNAL TABLE", ProjectSqlSymbolKind::Table),
        ("CREATE VIEW", ProjectSqlSymbolKind::View),
        ("CREATE OR REPLACE VIEW", ProjectSqlSymbolKind::View),
        ("CREATE MATERIALIZED VIEW", ProjectSqlSymbolKind::View),
        (
            "CREATE OR REPLACE MATERIALIZED VIEW",
            ProjectSqlSymbolKind::View,
        ),
        ("CREATE FUNCTION", ProjectSqlSymbolKind::Function),
        ("CREATE OR REPLACE FUNCTION", ProjectSqlSymbolKind::Function),
        ("CREATE PROCEDURE", ProjectSqlSymbolKind::Procedure),
        (
            "CREATE OR REPLACE PROCEDURE",
            ProjectSqlSymbolKind::Procedure,
        ),
    ];
    for (phrase, kind) in definitions {
        for start in project_keyword_phrase_starts(&upper, phrase) {
            let mut name_start = start + phrase.len();
            name_start =
                skip_project_keyword_sequence(source, &upper, name_start, &["IF", "NOT", "EXISTS"]);
            push_project_sql_occurrence(
                source,
                name_start,
                kind,
                ProjectSqlSymbolRole::Definition,
                &cte_scopes,
                &mut occurrences,
            );
        }
    }

    let references = [
        ("ALTER TABLE", ProjectSqlSymbolKind::Table),
        ("DROP TABLE", ProjectSqlSymbolKind::Table),
        ("TRUNCATE TABLE", ProjectSqlSymbolKind::Table),
        ("ALTER VIEW", ProjectSqlSymbolKind::View),
        ("DROP VIEW", ProjectSqlSymbolKind::View),
        ("REFRESH MATERIALIZED VIEW", ProjectSqlSymbolKind::View),
        ("DROP MATERIALIZED VIEW", ProjectSqlSymbolKind::View),
        ("INSERT INTO", ProjectSqlSymbolKind::Table),
        ("MERGE INTO", ProjectSqlSymbolKind::Table),
        ("DELETE FROM", ProjectSqlSymbolKind::Table),
        ("REFERENCES", ProjectSqlSymbolKind::Table),
        ("CALL", ProjectSqlSymbolKind::Procedure),
    ];
    for (phrase, kind) in references {
        for start in project_keyword_phrase_starts(&upper, phrase) {
            let mut name_start = start + phrase.len();
            name_start =
                skip_project_keyword_sequence(source, &upper, name_start, &["IF", "EXISTS"]);
            name_start = skip_project_relation_modifiers(source, &upper, name_start);
            push_project_sql_occurrence(
                source,
                name_start,
                kind,
                ProjectSqlSymbolRole::Reference,
                &cte_scopes,
                &mut occurrences,
            );
        }
    }

    for start in local_relation_source_starts(&upper) {
        let name_start = skip_project_relation_modifiers(source, &upper, start);
        push_project_sql_occurrence(
            source,
            name_start,
            ProjectSqlSymbolKind::Table,
            ProjectSqlSymbolRole::Reference,
            &cte_scopes,
            &mut occurrences,
        );
    }

    for start in project_keyword_phrase_starts(&upper, "UPDATE") {
        if project_update_is_relation_target(&upper, start) {
            let name_start =
                skip_project_relation_modifiers(source, &upper, start + "UPDATE".len());
            push_project_sql_occurrence(
                source,
                name_start,
                ProjectSqlSymbolKind::Table,
                ProjectSqlSymbolRole::Reference,
                &cte_scopes,
                &mut occurrences,
            );
        }
    }

    append_project_function_call_occurrences(source, &searchable, &mut occurrences);

    occurrences.sort_by_key(|occurrence| {
        (
            occurrence.range.start.line,
            occurrence.range.start.character,
            occurrence.range.end.line,
            occurrence.range.end.character,
            occurrence.role == ProjectSqlSymbolRole::Reference,
        )
    });
    occurrences.dedup_by(|left, right| {
        left.range == right.range && left.role == right.role && left.kind == right.kind
    });
    occurrences.truncate(PROJECT_SQL_INDEX_MAX_OCCURRENCES);
    occurrences
}

pub(super) fn project_keyword_phrase_starts(source_upper: &str, phrase: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut search = 0;
    while let Some(relative) = source_upper[search..].find(phrase) {
        let start = search + relative;
        if local_keyword_boundary(source_upper, start, phrase) {
            starts.push(start);
        }
        search = start + phrase.len();
    }
    starts
}

pub(super) fn skip_project_keyword_sequence(
    source: &str,
    source_upper: &str,
    start: usize,
    keywords: &[&str],
) -> usize {
    let original = skip_local_whitespace(source, start);
    let mut cursor = original;
    for keyword in keywords {
        cursor = skip_local_whitespace(source, cursor);
        if !source_upper[cursor..].starts_with(keyword)
            || !local_keyword_boundary(source_upper, cursor, keyword)
        {
            return original;
        }
        cursor += keyword.len();
    }
    skip_local_whitespace(source, cursor)
}

pub(super) fn skip_project_relation_modifiers(
    source: &str,
    source_upper: &str,
    start: usize,
) -> usize {
    let mut cursor = skip_local_whitespace(source, start);
    loop {
        let mut advanced = false;
        for keyword in ["ONLY", "LATERAL"] {
            if source_upper[cursor..].starts_with(keyword)
                && local_keyword_boundary(source_upper, cursor, keyword)
            {
                cursor = skip_local_whitespace(source, cursor + keyword.len());
                advanced = true;
                break;
            }
        }
        if !advanced {
            return cursor;
        }
    }
}

pub(super) fn read_project_identifier_path_after(
    source: &str,
    start: usize,
) -> Option<(String, String, usize, usize, usize)> {
    let (first, first_start, mut path_end) = read_local_identifier_after(source, start)?;
    let mut parts = vec![first];
    let (mut selection_start, mut selection_end) =
        project_identifier_selection_range(source, first_start, path_end);
    loop {
        let dot = skip_local_whitespace(source, path_end);
        if !source[dot..].starts_with('.') {
            break;
        }
        let after_dot = skip_local_whitespace(source, dot + 1);
        if source[after_dot..].starts_with('.') {
            parts.push("dbo".to_string());
            path_end = after_dot;
            continue;
        }
        let Some((part, part_start, part_end)) = read_local_identifier_after(source, after_dot)
        else {
            break;
        };
        parts.push(part);
        (selection_start, selection_end) =
            project_identifier_selection_range(source, part_start, part_end);
        path_end = part_end;
    }
    let path = parts.join(".");
    let name = parts.last()?.clone();
    Some((path, name, selection_start, selection_end, path_end))
}

pub(super) fn project_identifier_selection_range(
    source: &str,
    start: usize,
    end: usize,
) -> (usize, usize) {
    let Some(first) = source[start..end].chars().next() else {
        return (start, end);
    };
    let Some(last) = source[start..end].chars().next_back() else {
        return (start, end);
    };
    if matches!((first, last), ('"', '"') | ('`', '`') | ('[', ']')) {
        (start + first.len_utf8(), end - last.len_utf8())
    } else {
        (start, end)
    }
}

pub(super) fn push_project_sql_occurrence(
    source: &str,
    start: usize,
    kind: ProjectSqlSymbolKind,
    role: ProjectSqlSymbolRole,
    cte_scopes: &[(String, usize, usize)],
    occurrences: &mut Vec<ProjectSqlSymbolOccurrence>,
) {
    if occurrences.len() >= PROJECT_SQL_INDEX_MAX_OCCURRENCES || source[start..].starts_with('(') {
        return;
    }
    let Some((path, name, selection_start, selection_end, _)) =
        read_project_identifier_path_after(source, start)
    else {
        return;
    };
    if Keywords::is_keyword(&name) {
        return;
    }
    let normalized_name = SqlParser::normalize_identifier(&path).to_ascii_lowercase();
    if role == ProjectSqlSymbolRole::Reference
        && !kind.is_routine()
        && !normalized_name.contains('.')
        && cte_scopes.iter().any(|(cte, scope_start, scope_end)| {
            cte == &normalized_name
                && selection_start >= *scope_start
                && selection_start < *scope_end
        })
    {
        return;
    }
    occurrences.push(ProjectSqlSymbolOccurrence {
        name,
        normalized_name,
        kind,
        role,
        range: range_for_offsets(source, selection_start, selection_end),
    });
}

pub(super) fn append_project_function_call_occurrences(
    source: &str,
    searchable: &str,
    occurrences: &mut Vec<ProjectSqlSymbolOccurrence>,
) {
    let mut cursor = 0;
    while cursor < searchable.len() && occurrences.len() < PROJECT_SQL_INDEX_MAX_OCCURRENCES {
        let Some(character) = searchable[cursor..].chars().next() else {
            break;
        };
        let previous = searchable[..cursor].chars().next_back();
        if !(character == '_' || character.is_alphabetic())
            || previous.is_some_and(|value| value == '_' || value == '$' || value.is_alphanumeric())
        {
            cursor += character.len_utf8();
            continue;
        }
        let Some((path, name, selection_start, selection_end, path_end)) =
            read_project_identifier_path_after(source, cursor)
        else {
            cursor += character.len_utf8();
            continue;
        };
        cursor = path_end.max(cursor + character.len_utf8());
        let after_path = skip_local_whitespace(source, path_end);
        if !source[after_path..].starts_with('(') || Keywords::is_keyword(&name) {
            continue;
        }
        let range = range_for_offsets(source, selection_start, selection_end);
        if occurrences.iter().any(|occurrence| {
            occurrence.range == range && occurrence.role == ProjectSqlSymbolRole::Definition
        }) {
            continue;
        }
        if occurrences.iter().any(|occurrence| {
            occurrence.range == range
                && occurrence.role == ProjectSqlSymbolRole::Reference
                && occurrence.kind.is_routine()
        }) {
            continue;
        }
        occurrences.retain(|occurrence| {
            !(occurrence.range == range
                && occurrence.role == ProjectSqlSymbolRole::Reference
                && !occurrence.kind.is_routine())
        });
        occurrences.push(ProjectSqlSymbolOccurrence {
            name,
            normalized_name: SqlParser::normalize_identifier(&path).to_ascii_lowercase(),
            kind: ProjectSqlSymbolKind::Function,
            role: ProjectSqlSymbolRole::Reference,
            range,
        });
    }
}

pub(super) fn project_cte_scopes(source: &str, searchable: &str) -> Vec<(String, usize, usize)> {
    let parser = SqlParser::new();
    let mut scopes = Vec::new();
    let mut statement_start = 0;
    for statement_end in searchable
        .match_indices(';')
        .map(|(position, _)| position + 1)
        .chain(std::iter::once(source.len()))
    {
        if statement_end <= statement_start {
            continue;
        }
        for cte in parser.extract_common_table_expressions(&source[statement_start..statement_end])
        {
            scopes.push((
                SqlParser::normalize_identifier(&cte).to_ascii_lowercase(),
                statement_start,
                statement_end,
            ));
        }
        statement_start = statement_end;
    }
    scopes
}

pub(super) fn project_update_is_relation_target(source_upper: &str, update_start: usize) -> bool {
    let statement_start = source_upper[..update_start]
        .rfind(';')
        .map_or(0, |position| position + 1);
    let prefix = source_upper[statement_start..update_start].trim();
    prefix.is_empty()
        || (prefix.starts_with("WITH") && !prefix.contains("ON DUPLICATE KEY"))
        || prefix.ends_with("EXPLAIN")
        || prefix.ends_with("EXPLAIN ANALYZE")
}
