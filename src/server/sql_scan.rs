//! Sql scan for the SQL language server.
pub(super) fn skip_insert_target_alias(text: &str, lower: &str, from: usize, end: usize) -> usize {
    let cursor = skip_sql_whitespace(text, from, end);
    if sql_keyword_at(lower, "as", cursor, end) {
        return read_insert_identifier_part(text, cursor + "as".len(), end)
            .map_or(cursor, |(_, next)| skip_sql_whitespace(text, next, end));
    }
    if lower[cursor..end].starts_with('(')
        || ["values", "select", "default", "overriding"]
            .iter()
            .any(|keyword| sql_keyword_at(lower, keyword, cursor, end))
    {
        return cursor;
    }
    read_insert_identifier_part(text, cursor, end)
        .map_or(cursor, |(_, next)| skip_sql_whitespace(text, next, end))
}

pub(super) fn read_insert_qualified_identifier(
    text: &str,
    from: usize,
    end: usize,
) -> Option<(String, usize)> {
    let (first, mut cursor) = read_insert_identifier_part(text, from, end)?;
    let mut parts = vec![first];
    loop {
        let dot = skip_sql_whitespace(text, cursor, end);
        if !text[dot..end].starts_with('.') {
            break;
        }
        let after_dot = skip_sql_whitespace(text, dot + 1, end);
        if text[after_dot..end].starts_with('.') {
            parts.push(String::new());
            cursor = after_dot;
            continue;
        }
        let (part, next) = read_insert_identifier_part(text, after_dot, end)?;
        parts.push(part);
        cursor = next;
    }
    Some((parts.join("."), cursor))
}

pub(super) fn read_insert_identifier_part(
    text: &str,
    from: usize,
    end: usize,
) -> Option<(String, usize)> {
    let start = skip_sql_whitespace(text, from, end);
    let opener = text[start..end].chars().next()?;
    if matches!(opener, '"' | '`' | '[') {
        let closer = if opener == '[' { ']' } else { opener };
        let mut cursor = start + opener.len_utf8();
        while cursor < end {
            let character = text[cursor..].chars().next()?;
            cursor += character.len_utf8();
            if character != closer {
                continue;
            }
            if text[cursor..end].starts_with(closer) {
                cursor += closer.len_utf8();
                continue;
            }
            return Some((text[start..cursor].to_string(), cursor));
        }
        return None;
    }
    if !(opener == '_' || opener == '$' || opener == '#' || opener.is_alphabetic()) {
        return None;
    }
    let mut cursor = start + opener.len_utf8();
    while cursor < end {
        let character = text[cursor..].chars().next()?;
        if !(character == '_'
            || character == '$'
            || character == '#'
            || character.is_alphanumeric())
        {
            break;
        }
        cursor += character.len_utf8();
    }
    Some((text[start..cursor].to_string(), cursor))
}

pub(super) fn matching_sql_paren(masked: &str, open: usize, end: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut cursor = open;
    while cursor < end {
        let character = masked[cursor..].chars().next()?;
        match character {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(cursor);
                }
            }
            _ => {}
        }
        cursor += character.len_utf8();
    }
    None
}

pub(super) fn find_sql_keyword(
    lower: &str,
    keyword: &str,
    from: usize,
    end: usize,
) -> Option<usize> {
    let mut cursor = from;
    while cursor < end {
        let offset = lower[cursor..end].find(keyword)?;
        let candidate = cursor + offset;
        if sql_keyword_at(lower, keyword, candidate, end) {
            return Some(candidate);
        }
        cursor = candidate + keyword.len();
    }
    None
}

pub(super) fn find_sql_keyword_at_depth(
    lower: &str,
    keyword: &str,
    from: usize,
    end: usize,
    target_depth: usize,
) -> Option<usize> {
    let mut depth = 0usize;
    let mut cursor = from;
    while cursor < end {
        if depth == target_depth && sql_keyword_at(lower, keyword, cursor, end) {
            return Some(cursor);
        }
        let character = lower[cursor..].chars().next()?;
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        cursor += character.len_utf8();
    }
    None
}

pub(super) fn sql_keyword_at(lower: &str, keyword: &str, start: usize, end: usize) -> bool {
    let keyword_end = start.saturating_add(keyword.len());
    keyword_end <= end
        && lower[start..keyword_end].eq_ignore_ascii_case(keyword)
        && lower[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !is_insert_identifier_character(character))
        && lower[keyword_end..]
            .chars()
            .next()
            .is_none_or(|character| !is_insert_identifier_character(character))
}

pub(super) fn is_insert_identifier_character(character: char) -> bool {
    character == '_' || character == '$' || character == '#' || character.is_alphanumeric()
}

pub(super) fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset = offset.saturating_sub(1);
    }
    offset
}

pub(super) fn ceil_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset < text.len() && !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

pub(super) fn skip_sql_whitespace(text: &str, start: usize, end: usize) -> usize {
    let mut offset = start.min(end);
    while offset < end {
        let Some(character) = text[offset..end].chars().next() else {
            break;
        };
        if !character.is_whitespace() {
            break;
        }
        offset += character.len_utf8();
    }
    offset
}
