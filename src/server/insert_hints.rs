//! Insert hints for the SQL language server.
use super::completion_context::position_to_byte_offset;
use super::document_structure::position_for_offset;
use super::limits::{
    INLAY_HINT_MAX_ITEMS, INLAY_HINT_REQUEST_MAX_BYTES, INLAY_HINT_STATEMENT_SCAN_BYTES,
};
use super::schema_resolution::schema_table_matches;
use super::sql_scan::{
    ceil_char_boundary, find_sql_keyword, find_sql_keyword_at_depth, floor_char_boundary,
    matching_sql_paren, read_insert_qualified_identifier, skip_insert_target_alias,
    skip_sql_whitespace, sql_keyword_at,
};
use crate::parser::SqlParser;
use crate::schema::Schema;
use tower_lsp::lsp_types::*;
pub(super) fn insert_value_hints(text: &str, range: Range, schema: &Schema) -> Vec<InlayHint> {
    let absolute_range_start = position_to_byte_offset(text, range.start);
    let absolute_range_end = position_to_byte_offset(text, range.end)
        .max(absolute_range_start)
        .min(absolute_range_start.saturating_add(INLAY_HINT_REQUEST_MAX_BYTES));
    let provisional_window_start = floor_char_boundary(
        text,
        absolute_range_start.saturating_sub(INLAY_HINT_STATEMENT_SCAN_BYTES),
    );
    let mut window_start = if provisional_window_start == 0 {
        0
    } else {
        text[provisional_window_start..absolute_range_start]
            .find('\n')
            .map_or(provisional_window_start, |offset| {
                provisional_window_start + offset + 1
            })
    };
    let prefix = &text[window_start..absolute_range_start];
    if let Some(relative_body_start) = enclosing_dollar_quote_body_start(prefix, prefix.len()) {
        window_start += relative_body_start;
    }
    let window_end = ceil_char_boundary(
        text,
        absolute_range_end
            .saturating_add(INLAY_HINT_STATEMENT_SCAN_BYTES)
            .min(text.len()),
    );
    let text_window = &text[window_start..window_end];
    let range_start = absolute_range_start - window_start;
    let range_end = absolute_range_end - window_start;
    let masked = SqlParser::mask_sql_noise(text_window);
    let lower = masked.to_ascii_lowercase();
    let scan_start = lower[..range_start]
        .rfind(';')
        .map_or(0, |offset| offset + 1);
    let scan_end = lower[range_end..]
        .find(';')
        .map_or(text_window.len(), |offset| range_end + offset);
    let mut labels = Vec::new();
    let mut search_from = scan_start;

    while search_from < scan_end && labels.len() < INLAY_HINT_MAX_ITEMS {
        let Some(insert) = find_sql_keyword(&lower, "insert", search_from, scan_end) else {
            break;
        };
        let statement_end = lower[insert..scan_end]
            .find(';')
            .map_or(scan_end, |offset| insert + offset);
        let Some(into) = find_sql_keyword(&lower, "into", insert + "insert".len(), statement_end)
        else {
            search_from = insert + "insert".len();
            continue;
        };
        let Some((table_reference, mut cursor)) =
            read_insert_qualified_identifier(text_window, into + "into".len(), statement_end)
        else {
            search_from = into + "into".len();
            continue;
        };

        cursor = skip_sql_whitespace(text_window, cursor, statement_end);
        if sql_keyword_at(&lower, "with", cursor, statement_end) {
            let after_with = skip_sql_whitespace(text_window, cursor + "with".len(), statement_end);
            if lower[after_with..statement_end].starts_with('(') {
                if let Some(close) = matching_sql_paren(&masked, after_with, statement_end) {
                    cursor = skip_sql_whitespace(text_window, close + 1, statement_end);
                }
            }
        }
        cursor = skip_insert_target_alias(text_window, &lower, cursor, statement_end);

        let (columns, values_search_from) = if lower[cursor..statement_end].starts_with('(') {
            let Some(close) = matching_sql_paren(&masked, cursor, statement_end) else {
                search_from = statement_end.max(insert + "insert".len());
                continue;
            };
            (
                Some(insert_column_names(text_window, &masked, cursor + 1, close)),
                close + 1,
            )
        } else {
            (None, cursor)
        };
        let values =
            find_sql_keyword_at_depth(&lower, "values", values_search_from, statement_end, 0);
        let select = find_insert_source_select(
            text_window,
            &masked,
            &lower,
            values_search_from,
            statement_end,
        );
        if values.is_none() && select.is_none() {
            search_from = statement_end.max(insert + "insert".len());
            continue;
        }

        let resolved_columns = columns.unwrap_or_else(|| {
            schema
                .tables
                .iter()
                .find(|table| schema_table_matches(schema, &table_reference, table))
                .map(|table| {
                    table
                        .columns
                        .iter()
                        .filter(|column| {
                            !column.auto_increment && !column.generated && !column.hidden
                        })
                        .map(|column| column.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        });
        if !resolved_columns.is_empty() {
            let use_select =
                select.is_some_and(|(select, _)| values.is_none_or(|values| select < values));
            if use_select {
                if let Some((select, depth)) = select {
                    append_insert_select_hints(
                        InsertSelectProjection {
                            text: text_window,
                            masked: &masked,
                            lower: &lower,
                            select,
                            depth,
                            end: statement_end,
                        },
                        (range_start, range_end),
                        &resolved_columns,
                        &mut labels,
                    );
                }
            } else if let Some(values) = values {
                append_insert_row_hints(
                    text_window,
                    &masked,
                    (values + "values".len(), statement_end),
                    (range_start, range_end),
                    &resolved_columns,
                    &mut labels,
                );
            }
        }
        search_from = statement_end.max(insert + "insert".len());
    }

    labels
        .into_iter()
        .map(|(offset, column)| InlayHint {
            position: position_for_offset(text, window_start + offset),
            label: InlayHintLabel::String(format!("{column}:")),
            kind: Some(InlayHintKind::PARAMETER),
            text_edits: None,
            tooltip: Some(InlayHintTooltip::String(
                "Column matched from the INSERT target and value position.".to_string(),
            )),
            padding_left: None,
            padding_right: Some(true),
            data: None,
        })
        .collect()
}

pub(super) fn enclosing_dollar_quote_body_start(text: &str, position: usize) -> Option<usize> {
    let end = floor_char_boundary(text, position);
    let mut cursor = 0usize;
    let mut dollar_quote: Option<(String, usize)> = None;
    let mut quote: Option<char> = None;
    let mut line_comment = false;
    let mut block_comment = false;

    while cursor < end {
        if let Some((tag, _)) = dollar_quote.as_ref() {
            if text[cursor..end].starts_with(tag) {
                cursor += tag.len();
                dollar_quote = None;
                continue;
            }
            cursor += text[cursor..].chars().next()?.len_utf8();
            continue;
        }
        if line_comment {
            let character = text[cursor..].chars().next()?;
            cursor += character.len_utf8();
            if character == '\n' {
                line_comment = false;
            }
            continue;
        }
        if block_comment {
            if text[cursor..end].starts_with("*/") {
                cursor += 2;
                block_comment = false;
            } else {
                cursor += text[cursor..].chars().next()?.len_utf8();
            }
            continue;
        }
        if let Some(active_quote) = quote {
            let character = text[cursor..].chars().next()?;
            cursor += character.len_utf8();
            if character == active_quote {
                if text[cursor..end].starts_with(active_quote) {
                    cursor += active_quote.len_utf8();
                } else {
                    quote = None;
                }
            }
            continue;
        }
        if text[cursor..end].starts_with("--") {
            cursor += 2;
            line_comment = true;
            continue;
        }
        if text[cursor..end].starts_with("/*") {
            cursor += 2;
            block_comment = true;
            continue;
        }
        let character = text[cursor..].chars().next()?;
        if matches!(character, '\'' | '"' | '`') {
            quote = Some(character);
            cursor += character.len_utf8();
            continue;
        }
        if character == '$' {
            if let Some(tag) = insert_dollar_quote_tag_at(text, cursor, end) {
                let body_start = cursor + tag.len();
                dollar_quote = Some((tag.to_string(), body_start));
                cursor = body_start;
                continue;
            }
        }
        cursor += character.len_utf8();
    }

    dollar_quote.map(|(_, body_start)| body_start)
}

pub(super) fn insert_dollar_quote_tag_at(text: &str, start: usize, end: usize) -> Option<&str> {
    let bytes = text.get(start..end)?.as_bytes();
    if bytes.first() != Some(&b'$') {
        return None;
    }
    let mut offset = 1usize;
    while offset < bytes.len() {
        let byte = bytes[offset];
        if byte == b'$' {
            return text.get(start..start + offset + 1);
        }
        if (offset == 1 && !(byte == b'_' || byte.is_ascii_alphabetic()))
            || (offset > 1 && !(byte == b'_' || byte.is_ascii_alphanumeric()))
        {
            return None;
        }
        offset += 1;
        if offset > 128 {
            return None;
        }
    }
    None
}

pub(super) struct InsertSelectProjection<'a> {
    pub(super) text: &'a str,
    pub(super) masked: &'a str,
    pub(super) lower: &'a str,
    pub(super) select: usize,
    pub(super) depth: usize,
    pub(super) end: usize,
}

pub(super) fn append_insert_select_hints(
    source: InsertSelectProjection<'_>,
    requested_range: (usize, usize),
    columns: &[String],
    hints: &mut Vec<(usize, String)>,
) {
    let offsets = insert_select_projection_offsets(
        source.text,
        source.masked,
        source.lower,
        source.select,
        source.depth,
        source.end,
    );
    // A partial positional mapping is actively misleading. Unlike VALUES,
    // SELECT projections can be hidden behind wildcards or incomplete syntax,
    // so only label a projection when the entire target/source arity is known.
    if offsets.len() != columns.len() {
        return;
    }
    let (range_start, range_end) = requested_range;
    for (offset, column) in offsets.into_iter().zip(columns.iter()) {
        if offset < range_start || offset > range_end || hints.len() >= INLAY_HINT_MAX_ITEMS {
            continue;
        }
        hints.push((offset, column.clone()));
    }
}

pub(super) fn find_insert_source_select(
    text: &str,
    masked: &str,
    lower: &str,
    from: usize,
    end: usize,
) -> Option<(usize, usize)> {
    let source_start = skip_sql_whitespace(text, from, end);
    if masked[source_start..end].starts_with('(') {
        let inner = skip_sql_whitespace(text, source_start + 1, end);
        if sql_keyword_at(lower, "select", inner, end) {
            return Some((inner, 1));
        }
    }
    find_sql_keyword_at_depth(lower, "select", from, end, 0).map(|offset| (offset, 0))
}

pub(super) fn insert_select_projection_offsets(
    text: &str,
    masked: &str,
    lower: &str,
    select: usize,
    select_depth: usize,
    end: usize,
) -> Vec<usize> {
    let mut cursor = skip_sql_whitespace(text, select + "select".len(), end);
    if sql_keyword_at(lower, "all", cursor, end) {
        cursor = skip_sql_whitespace(text, cursor + "all".len(), end);
    } else if sql_keyword_at(lower, "distinct", cursor, end) {
        cursor = skip_sql_whitespace(text, cursor + "distinct".len(), end);
        if sql_keyword_at(lower, "on", cursor, end) {
            let open = skip_sql_whitespace(text, cursor + "on".len(), end);
            if !masked[open..end].starts_with('(') {
                return Vec::new();
            }
            let Some(close) = matching_sql_paren(masked, open, end) else {
                return Vec::new();
            };
            cursor = skip_sql_whitespace(text, close + 1, end);
        }
    }

    if sql_keyword_at(lower, "top", cursor, end) {
        cursor = skip_sql_whitespace(text, cursor + "top".len(), end);
        if masked[cursor..end].starts_with('(') {
            let Some(close) = matching_sql_paren(masked, cursor, end) else {
                return Vec::new();
            };
            cursor = skip_sql_whitespace(text, close + 1, end);
        } else {
            let value_start = cursor;
            while cursor < end {
                let Some(character) = masked[cursor..].chars().next() else {
                    break;
                };
                if character.is_whitespace() || character == ',' {
                    break;
                }
                cursor += character.len_utf8();
            }
            if cursor == value_start {
                return Vec::new();
            }
            cursor = skip_sql_whitespace(text, cursor, end);
        }
        if sql_keyword_at(lower, "percent", cursor, end) {
            cursor = skip_sql_whitespace(text, cursor + "percent".len(), end);
        }
        if sql_keyword_at(lower, "with", cursor, end) {
            let ties = skip_sql_whitespace(text, cursor + "with".len(), end);
            if sql_keyword_at(lower, "ties", ties, end) {
                cursor = skip_sql_whitespace(text, ties + "ties".len(), end);
            }
        }
    }

    let mut offsets = Vec::new();
    let mut expression_start = cursor;
    let mut depth = select_depth;
    while cursor < end {
        let Some(character) = masked[cursor..].chars().next() else {
            break;
        };
        if depth == select_depth
            && SELECT_PROJECTION_BOUNDARIES
                .iter()
                .any(|keyword| sql_keyword_at(lower, keyword, cursor, end))
        {
            break;
        }
        match character {
            '(' => depth += 1,
            ')' if depth == select_depth => break,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == select_depth => {
                let Some(offset) =
                    first_insert_value_offset(text, masked, expression_start, cursor)
                else {
                    return Vec::new();
                };
                if insert_select_projection_is_wildcard(masked, expression_start, cursor) {
                    return Vec::new();
                }
                offsets.push(offset);
                expression_start = cursor + 1;
            }
            _ => {}
        }
        cursor += character.len_utf8();
    }
    let Some(offset) = first_insert_value_offset(text, masked, expression_start, cursor) else {
        return Vec::new();
    };
    if insert_select_projection_is_wildcard(masked, expression_start, cursor) {
        return Vec::new();
    }
    offsets.push(offset);
    offsets
}

pub(super) const SELECT_PROJECTION_BOUNDARIES: [&str; 16] = [
    "from",
    "into",
    "where",
    "group",
    "having",
    "order",
    "offset",
    "fetch",
    "for",
    "option",
    "union",
    "except",
    "intersect",
    "limit",
    "qualify",
    "window",
];

pub(super) fn insert_select_projection_is_wildcard(masked: &str, from: usize, to: usize) -> bool {
    let mut compact = String::new();
    let mut depth = 0usize;
    let mut cursor = from;
    while cursor < to {
        let Some(character) = masked[cursor..].chars().next() else {
            break;
        };
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 && !character.is_whitespace() => compact.push(character),
            _ => {}
        }
        cursor += character.len_utf8();
    }
    if compact == "*" || compact == ".*" {
        return true;
    }
    let Some(qualifier) = compact.strip_suffix(".*") else {
        return false;
    };
    !qualifier.is_empty()
        && qualifier.chars().all(|character| {
            character == '_'
                || character == '$'
                || character == '#'
                || character == '.'
                || character == '['
                || character == ']'
                || character.is_alphanumeric()
        })
}

pub(super) fn append_insert_row_hints(
    text: &str,
    masked: &str,
    statement: (usize, usize),
    requested_range: (usize, usize),
    columns: &[String],
    hints: &mut Vec<(usize, String)>,
) {
    let (from, statement_end) = statement;
    let (range_start, range_end) = requested_range;
    let mut cursor = skip_sql_whitespace(text, from, statement_end);
    while cursor < statement_end
        && masked[cursor..statement_end].starts_with('(')
        && hints.len() < INLAY_HINT_MAX_ITEMS
    {
        let Some((value_offsets, next)) =
            insert_value_row_offsets(text, masked, cursor, statement_end)
        else {
            break;
        };
        for (offset, column) in value_offsets.into_iter().zip(columns.iter()) {
            if offset < range_start || offset > range_end || hints.len() >= INLAY_HINT_MAX_ITEMS {
                continue;
            }
            hints.push((offset, column.clone()));
        }
        cursor = skip_sql_whitespace(text, next, statement_end);
        if !masked[cursor..statement_end].starts_with(',') {
            break;
        }
        cursor = skip_sql_whitespace(text, cursor + 1, statement_end);
    }
}

pub(super) fn insert_value_row_offsets(
    text: &str,
    masked: &str,
    open: usize,
    end: usize,
) -> Option<(Vec<usize>, usize)> {
    let mut offsets = Vec::new();
    let mut depth = 1usize;
    let mut value_start = open + 1;
    let mut cursor = open + 1;
    while cursor < end {
        let character = masked[cursor..].chars().next()?;
        match character {
            '(' => depth += 1,
            ')' if depth == 1 => {
                if let Some(offset) = first_insert_value_offset(text, masked, value_start, cursor) {
                    offsets.push(offset);
                }
                return Some((offsets, cursor + 1));
            }
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 1 => {
                if let Some(offset) = first_insert_value_offset(text, masked, value_start, cursor) {
                    offsets.push(offset);
                }
                value_start = cursor + 1;
            }
            ';' if depth == 1 => return None,
            _ => {}
        }
        cursor += character.len_utf8();
    }
    None
}

pub(super) fn first_insert_value_offset(
    text: &str,
    masked: &str,
    from: usize,
    to: usize,
) -> Option<usize> {
    let mut cursor = from;
    while cursor < to {
        let raw = text[cursor..].chars().next()?;
        let searchable = masked[cursor..].chars().next()?;
        if !searchable.is_whitespace() || matches!(raw, '\'' | '"' | '`' | '[' | '$') {
            return Some(cursor);
        }
        cursor += raw.len_utf8();
    }
    None
}

pub(super) fn insert_column_names(text: &str, masked: &str, from: usize, to: usize) -> Vec<String> {
    let mut columns = Vec::new();
    let mut start = from;
    let mut depth = 0usize;
    let mut cursor = from;
    while cursor <= to {
        let character = if cursor == to {
            ','
        } else {
            masked[cursor..].chars().next().unwrap_or(',')
        };
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                let Some(column) = normalize_insert_identifier(&text[start..cursor]) else {
                    return Vec::new();
                };
                columns.push(column);
                start = cursor + 1;
            }
            _ => {}
        }
        cursor += character.len_utf8();
    }
    columns
}

pub(super) fn normalize_insert_identifier(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        return Some(trimmed[1..trimmed.len() - 1].replace("]]", "]"));
    }
    for quote in ['"', '`'] {
        if trimmed.starts_with(quote) && trimmed.ends_with(quote) {
            return Some(
                trimmed[quote.len_utf8()..trimmed.len() - quote.len_utf8()]
                    .replace(&format!("{quote}{quote}"), &quote.to_string()),
            );
        }
    }
    trimmed
        .chars()
        .all(|character| character == '_' || character == '$' || character.is_alphanumeric())
        .then(|| trimmed.to_string())
}
