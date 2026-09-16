//! Document structure for the SQL language server.
use super::completion_context::line_bounds_for_position;
use super::locations::identifier_range_at_position;
use super::project_types::{ProjectSqlSymbolOccurrence, ProjectSqlSymbolRole};
use crate::parser::SqlParser;
use crate::position::lsp_position_at_end;
use crate::schema::Schema;
use std::collections::HashSet;
use tower_lsp::lsp_types::*;
#[allow(deprecated)]
pub(super) fn document_symbols(
    text: &str,
    schema: Option<&Schema>,
    project_occurrences: &[ProjectSqlSymbolOccurrence],
) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();
    let mut seen = HashSet::new();
    for occurrence in project_occurrences
        .iter()
        .filter(|occurrence| occurrence.role == ProjectSqlSymbolRole::Definition)
    {
        let category = if occurrence.kind.is_routine() {
            "routine"
        } else {
            "relation"
        };
        if seen.insert((category, occurrence.name.to_ascii_lowercase())) {
            symbols.push(DocumentSymbol {
                name: occurrence.name.clone(),
                detail: Some(occurrence.kind.detail().to_string()),
                kind: occurrence.kind.symbol_kind(),
                tags: None,
                deprecated: None,
                range: occurrence.range,
                selection_range: occurrence.range,
                children: None,
            });
        }
    }
    if let Some(schema) = schema {
        for table in &schema.tables {
            let range = project_occurrences
                .iter()
                .find(|occurrence| {
                    !occurrence.kind.is_routine()
                        && occurrence.name.eq_ignore_ascii_case(&table.name)
                })
                .map(|occurrence| occurrence.range)
                .or_else(|| range_for_identifier_occurrence(text, &table.name));
            if let Some(range) = range {
                if seen.insert(("relation", table.name.to_ascii_lowercase())) {
                    symbols.push(DocumentSymbol {
                        name: table.name.clone(),
                        detail: Some(table.object_kind().to_string()),
                        kind: SymbolKind::CLASS,
                        tags: None,
                        deprecated: None,
                        range,
                        selection_range: range,
                        children: None,
                    });
                }
            }
        }
        for function in &schema.functions {
            let range = project_occurrences
                .iter()
                .find(|occurrence| {
                    occurrence.kind.is_routine()
                        && occurrence.name.eq_ignore_ascii_case(&function.name)
                })
                .map(|occurrence| occurrence.range)
                .or_else(|| range_for_identifier_occurrence(text, &function.name));
            if let Some(range) = range {
                if seen.insert(("routine", function.name.to_ascii_lowercase())) {
                    symbols.push(DocumentSymbol {
                        name: function.name.clone(),
                        detail: Some(function.signature()),
                        kind: SymbolKind::FUNCTION,
                        tags: None,
                        deprecated: None,
                        range,
                        selection_range: range,
                        children: None,
                    });
                }
            }
        }
    }
    let parser = SqlParser::new();
    for cte in parser.extract_common_table_expressions(text) {
        if let Some(range) = range_for_identifier_occurrence(text, &cte) {
            if seen.insert(("cte", cte.to_ascii_lowercase())) {
                symbols.push(DocumentSymbol {
                    name: cte,
                    detail: Some("Common table expression".to_string()),
                    kind: SymbolKind::VARIABLE,
                    tags: None,
                    deprecated: None,
                    range,
                    selection_range: range,
                    children: None,
                });
            }
        }
    }
    symbols.sort_by_key(|symbol| {
        (
            symbol.range.start.line,
            symbol.range.start.character,
            symbol.name.clone(),
        )
    });
    symbols
}

pub(super) fn range_for_identifier_occurrence(text: &str, identifier: &str) -> Option<Range> {
    let searchable = text.to_ascii_lowercase();
    let needle = identifier.to_ascii_lowercase();
    searchable
        .match_indices(&needle)
        .find_map(|(start, matched)| {
            let end = start + matched.len();
            let before = text[..start].chars().next_back();
            let after = text[end..].chars().next();
            if before.is_some_and(|character| character.is_alphanumeric() || character == '_')
                || after.is_some_and(|character| character.is_alphanumeric() || character == '_')
            {
                return None;
            }
            Some(range_for_offsets(text, start, end))
        })
}

pub(super) fn range_for_offsets(text: &str, start: usize, end: usize) -> Range {
    Range {
        start: position_for_offset(text, start),
        end: position_for_offset(text, end),
    }
}

pub(super) fn position_for_offset(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let prefix = &text[..offset];
    let line = prefix.matches('\n').count() as u32;
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    Position {
        line,
        character: text[line_start..offset].encode_utf16().count() as u32,
    }
}

pub(super) fn folding_ranges(text: &str) -> Vec<FoldingRange> {
    let mut stack = Vec::new();
    let mut ranges = Vec::new();
    let mut line = 0u32;
    let mut character = 0u32;
    let mut quote = None;
    for current in text.chars() {
        if current == '\n' {
            line += 1;
            character = 0;
            continue;
        }
        if let Some(active_quote) = quote {
            if current == active_quote {
                quote = None;
            }
            character += current.len_utf16() as u32;
            continue;
        }
        if matches!(current, '\'' | '"' | '`') {
            quote = Some(current);
            character += 1;
            continue;
        }
        match current {
            '(' | '[' | '{' => stack.push((current, line, character)),
            ')' | ']' | '}' => {
                let expected = match current {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                if let Some(index) = stack.iter().rposition(|(open, _, _)| *open == expected) {
                    let (_, start_line, start_character) = stack.remove(index);
                    if start_line < line {
                        ranges.push(FoldingRange {
                            start_line,
                            start_character: Some(start_character),
                            end_line: line,
                            end_character: Some(character),
                            kind: None,
                            collapsed_text: None,
                        });
                    }
                }
            }
            _ => {}
        }
        character += current.len_utf16() as u32;
    }
    ranges.sort_by_key(|range| (range.start_line, range.start_character));
    ranges.truncate(500);
    ranges
}

pub(super) fn selection_range_for_position(
    text: &str,
    position: Position,
    dialect: &str,
) -> SelectionRange {
    let document_range = Range::new(Position::new(0, 0), lsp_position_at_end(text));
    let (line_start, line_end) = line_bounds_for_position(text, position.line);
    let line_range = range_for_offsets(text, line_start, line_end);
    let parent = SelectionRange {
        range: line_range,
        parent: Some(Box::new(SelectionRange {
            range: document_range,
            parent: None,
        })),
    };
    SelectionRange {
        range: identifier_range_at_position(text, position, dialect)
            .unwrap_or(Range::new(position, position)),
        parent: Some(Box::new(parent)),
    }
}
