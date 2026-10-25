pub(in crate::server) fn read_local_identifier_after(
    source: &str,
    start: usize,
) -> Option<(String, usize, usize)> {
    let start = skip_local_whitespace(source, start);
    let first = source[start..].chars().next()?;
    if matches!(first, '"' | '`' | '[') {
        let closing = if first == '[' { ']' } else { first };
        let mut cursor = start + first.len_utf8();
        let mut name = String::new();
        while let Some(character) = source[cursor..].chars().next() {
            cursor += character.len_utf8();
            if character == closing {
                if source[cursor..].starts_with(closing) {
                    name.push(closing);
                    cursor += closing.len_utf8();
                    continue;
                }
                return Some((name, start, cursor));
            }
            name.push(character);
        }
        return None;
    }
    if !(first == '_' || first.is_alphabetic()) {
        return None;
    }
    let mut end = start + first.len_utf8();
    for character in source[end..].chars() {
        if !(character == '_' || character == '$' || character.is_alphanumeric()) {
            break;
        }
        end += character.len_utf8();
    }
    Some((source[start..end].to_string(), start, end))
}

pub(in crate::server) fn read_local_identifier_path_after(
    source: &str,
    start: usize,
) -> Option<(String, usize, usize)> {
    let (parts, start, end) = read_local_identifier_parts_after(source, start)?;
    Some((
        crate::parser::SqlParser::identifier_path_from_parts(&parts),
        start,
        end,
    ))
}

pub(in crate::server) fn read_local_identifier_parts_after(
    source: &str,
    start: usize,
) -> Option<(Vec<String>, usize, usize)> {
    let (first, path_start, mut path_end) = read_local_identifier_after(source, start)?;
    let mut parts = vec![first];
    loop {
        let dot = skip_local_whitespace(source, path_end);
        if !source[dot..].starts_with('.') {
            break;
        }
        let Some((part, _, part_end)) = read_local_identifier_after(source, dot + 1) else {
            break;
        };
        parts.push(part);
        path_end = part_end;
    }
    Some((parts, path_start, path_end))
}

pub(in crate::server) fn local_relation_source_starts(source_upper: &str) -> Vec<usize> {
    const FROM_BOUNDARIES: &[&str] = &[
        "WHERE",
        "GROUP",
        "HAVING",
        "ORDER",
        "LIMIT",
        "OFFSET",
        "FETCH",
        "WINDOW",
        "QUALIFY",
        "RETURNING",
        "UNION",
        "EXCEPT",
        "INTERSECT",
    ];
    let mut starts = Vec::new();
    for keyword in ["FROM", "JOIN", "APPLY"] {
        let mut search = 0;
        while let Some(relative) = source_upper[search..].find(keyword) {
            let position = search + relative;
            if local_keyword_boundary(source_upper, position, keyword) {
                starts.push(position + keyword.len());
            }
            search = position + keyword.len();
        }
    }
    let mut from_search = 0;
    while let Some(relative) = source_upper[from_search..].find("FROM") {
        let from_position = from_search + relative;
        if !local_keyword_boundary(source_upper, from_position, "FROM") {
            from_search = from_position + "FROM".len();
            continue;
        }
        let mut cursor = from_position + "FROM".len();
        let mut depth = 0u32;
        while cursor < source_upper.len() {
            let Some(character) = source_upper[cursor..].chars().next() else {
                break;
            };
            if depth == 0 {
                if matches!(character, ';' | ')')
                    || FROM_BOUNDARIES.iter().any(|keyword| {
                        source_upper[cursor..].starts_with(keyword)
                            && local_keyword_boundary(source_upper, cursor, keyword)
                    })
                {
                    break;
                }
                if character == ',' {
                    starts.push(cursor + character.len_utf8());
                }
            }
            match character {
                '(' => depth += 1,
                ')' if depth > 0 => depth -= 1,
                _ => {}
            }
            cursor += character.len_utf8();
        }
        from_search = (from_position + "FROM".len()).max(cursor);
    }
    starts.sort_unstable();
    starts.dedup();
    starts
}

pub(in crate::server) fn skip_local_whitespace(source: &str, mut offset: usize) -> usize {
    offset = offset.min(source.len());
    while offset < source.len() {
        let Some(character) = source[offset..].chars().next() else {
            break;
        };
        if !character.is_whitespace() {
            break;
        }
        offset += character.len_utf8();
    }
    offset
}

pub(in crate::server) fn skip_optional_keyword_sequence(
    source: &str,
    start: usize,
    keywords: &[&str],
) -> usize {
    let upper = source.to_ascii_uppercase();
    let original = skip_local_whitespace(source, start);
    let mut cursor = original;
    for keyword in keywords {
        if !upper[cursor..].starts_with(keyword) || !local_keyword_boundary(&upper, cursor, keyword)
        {
            return original;
        }
        cursor = skip_local_whitespace(source, cursor + keyword.len());
    }
    cursor
}

pub(in crate::server) fn local_keyword_boundary(
    source_upper: &str,
    start: usize,
    keyword: &str,
) -> bool {
    if keyword == "FROM" && crate::parser::SqlParser::is_distinct_from_keyword(source_upper, start)
    {
        return false;
    }
    let end = start + keyword.len();
    let before = source_upper[..start].chars().next_back();
    let after = source_upper[end.min(source_upper.len())..].chars().next();
    !before.is_some_and(|character| character.is_alphanumeric() || character == '_')
        && !after.is_some_and(|character| character.is_alphanumeric() || character == '_')
}

pub(in crate::server) fn matching_parenthesis_in_searchable(
    searchable: &str,
    opening: usize,
) -> Option<usize> {
    if !searchable[opening..].starts_with('(') {
        return None;
    }
    let mut nesting = 0u32;
    for (relative, character) in searchable[opening..].char_indices() {
        match character {
            '(' => nesting += 1,
            ')' => {
                nesting = nesting.saturating_sub(1);
                if nesting == 0 {
                    return Some(opening + relative);
                }
            }
            _ => {}
        }
    }
    None
}

pub(in crate::server) fn split_top_level_ranges_in_searchable(
    searchable: &str,
    start: usize,
    end: usize,
) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut segment_start = start;
    let mut nesting = 0u32;
    for (relative, character) in searchable[start..end].char_indices() {
        let offset = start + relative;
        match character {
            '(' | '[' | '{' => nesting += 1,
            ')' | ']' | '}' if nesting > 0 => nesting -= 1,
            ',' if nesting == 0 => {
                ranges.push((segment_start, offset));
                segment_start = offset + character.len_utf8();
            }
            _ => {}
        }
    }
    ranges.push((segment_start, end));
    ranges
}

pub(in crate::server) fn find_top_level_keyword_in_searchable(
    upper: &str,
    start: usize,
    keyword: &str,
) -> Option<usize> {
    let mut nesting = 0u32;
    for (relative, character) in upper[start..].char_indices() {
        let offset = start + relative;
        match character {
            '(' | '[' | '{' => nesting += 1,
            ')' | ']' | '}' if nesting > 0 => nesting -= 1,
            _ if nesting == 0
                && upper[offset..].starts_with(keyword)
                && local_keyword_boundary(upper, offset, keyword) =>
            {
                return Some(offset);
            }
            _ => {}
        }
    }
    None
}

pub(in crate::server) fn skip_sql_trivia(source: &str, mut offset: usize) -> usize {
    loop {
        offset = skip_local_whitespace(source, offset);
        if source[offset..].starts_with("--") {
            offset = source[offset..]
                .find('\n')
                .map_or(source.len(), |end| offset + end);
        } else if source[offset..].starts_with("/*") {
            let mut depth = 1;
            offset += 2;
            while offset < source.len() && depth > 0 {
                if source[offset..].starts_with("/*") {
                    depth += 1;
                    offset += 2;
                } else if source[offset..].starts_with("*/") {
                    depth -= 1;
                    offset += 2;
                } else {
                    offset += source[offset..].chars().next().unwrap().len_utf8();
                }
            }
        } else {
            return offset;
        }
    }
}
