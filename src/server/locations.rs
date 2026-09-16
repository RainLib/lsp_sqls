//! Locations for the SQL language server.
use super::completion_context::position_to_byte_offset;
use super::limits::{CURRENT_JSON_DOCUMENT_URI, CURRENT_SQL_DOCUMENT_URI};
use crate::parser::SqlParser;
use std::collections::HashSet;
use tower_lsp::lsp_types::*;
pub(super) fn rewrite_current_document_location_uri(location: &mut Location, document_uri: &Url) {
    if matches!(
        location.uri.as_str(),
        CURRENT_SQL_DOCUMENT_URI | CURRENT_JSON_DOCUMENT_URI
    ) {
        location.uri = document_uri.clone();
    }
}

pub(super) fn rewrite_current_document_location_uris(
    locations: &mut [Location],
    document_uri: &Url,
) {
    for location in locations {
        rewrite_current_document_location_uri(location, document_uri);
    }
}

pub(super) fn identifier_at_position(
    text: &str,
    position: Position,
    dialect: &str,
) -> Option<String> {
    let offset = position_to_byte_offset(text, position).min(text.len());
    let mut start = offset;
    while start > 0 {
        let ch = text[..start].chars().next_back()?;
        if !is_identifier_character(ch, dialect) {
            break;
        }
        start -= ch.len_utf8();
    }
    let mut end = offset;
    while end < text.len() {
        let ch = text[end..].chars().next()?;
        if !is_identifier_character(ch, dialect) {
            break;
        }
        end += ch.len_utf8();
    }
    let identifier = text[start..end]
        .trim_matches(|character| matches!(character, '"' | '\'' | '`'))
        .to_string();
    (!identifier.is_empty()).then_some(identifier)
}

pub(super) fn cte_identifier_range_at_position(
    text: &str,
    position: Position,
    dialect: &str,
) -> Option<Range> {
    let identifier = identifier_at_position(text, position, dialect)?;
    let parser = SqlParser::new();
    parser
        .extract_common_table_expressions(text)
        .into_iter()
        .any(|cte| SqlParser::identifier_last_part(&cte).eq_ignore_ascii_case(&identifier))
        .then(|| identifier_range_at_position(text, position, dialect))?
}

pub(super) fn is_identifier_character(character: char, dialect: &str) -> bool {
    character.is_alphanumeric()
        || character == '_'
        || match dialect {
            "redis" => matches!(character, ':' | '-' | '.' | '@'),
            "mongodb" | "elasticsearch-dsl" | "elasticsearch-eql" => {
                matches!(character, '.' | '-' | '@')
            }
            _ => character == '$',
        }
}

pub(super) fn deduplicate_locations(locations: &mut Vec<Location>) {
    let mut seen = HashSet::new();
    locations.retain(|location| {
        seen.insert((
            location.uri.to_string(),
            location.range.start.line,
            location.range.start.character,
            location.range.end.line,
            location.range.end.character,
        ))
    });
    locations.sort_by(|left, right| {
        left.uri
            .as_str()
            .cmp(right.uri.as_str())
            .then_with(|| left.range.start.line.cmp(&right.range.start.line))
            .then_with(|| left.range.start.character.cmp(&right.range.start.character))
    });
}

pub(super) fn supports_semantic_rename(dialect: &str) -> bool {
    matches!(dialect, "postgres" | "mysql" | "clickhouse" | "hive")
}

pub(super) fn workspace_symbol_matches(query: &str, name: &str, container: &str) -> bool {
    query.is_empty()
        || name.to_ascii_lowercase().contains(query)
        || container.to_ascii_lowercase().contains(query)
}

pub(super) fn workspace_symbol_rank(query: &str, name: &str) -> u8 {
    let name = name.to_ascii_lowercase();
    if query.is_empty() || name == query {
        0
    } else if name.starts_with(query) {
        1
    } else {
        2
    }
}

pub(super) fn range_contains_position(range: Range, position: Position) -> bool {
    position >= range.start && position <= range.end
}

pub(super) fn valid_renamed_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first == '_' || first.is_alphabetic())
        && characters.all(|character| character == '_' || character.is_alphanumeric())
}

pub(super) fn identifier_range_at_position(
    text: &str,
    position: Position,
    dialect: &str,
) -> Option<Range> {
    let identifier = identifier_at_position(text, position, dialect)?;
    let offset = position_to_byte_offset(text, position).min(text.len());
    let line_start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let line_end = text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index);
    let line = &text[line_start..line_end];
    let cursor = offset.saturating_sub(line_start).min(line.len());
    let searchable = line.to_ascii_lowercase();
    let needle = identifier.to_ascii_lowercase();
    let (start, matched) = searchable
        .match_indices(&needle)
        .find(|(start, matched)| *start <= cursor && cursor <= *start + matched.len())?;
    let line_number = text[..line_start].matches('\n').count() as u32;
    Some(Range {
        start: Position {
            line: line_number,
            character: line[..start].encode_utf16().count() as u32,
        },
        end: Position {
            line: line_number,
            character: line[..start + matched.len()].encode_utf16().count() as u32,
        },
    })
}
