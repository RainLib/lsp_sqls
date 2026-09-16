use crate::parser::SqlParser;
use tower_lsp::lsp_types::Position;

pub(crate) fn cursor_prefix(sql: &str, position: Position) -> String {
    let lines: Vec<&str> = sql.lines().collect();
    let line_text = lines.get(position.line as usize).unwrap_or(&"");
    let text_before = &line_text[..position.character.min(line_text.len() as u32) as usize];

    let token = cursor_identifier_token(text_before);

    if token.trim_end().ends_with('.') {
        return String::new();
    }

    SqlParser::identifier_last_part(token).to_lowercase()
}

pub(crate) fn cursor_prefix_excluding_keywords(
    sql: &str,
    position: Position,
    keywords: &[&str],
) -> String {
    let prefix = cursor_prefix(sql, position);
    if keywords
        .iter()
        .any(|keyword| prefix.eq_ignore_ascii_case(keyword))
    {
        String::new()
    } else {
        prefix
    }
}

pub(crate) fn cursor_has_identifier_qualifier(sql: &str, position: Position) -> bool {
    let lines: Vec<&str> = sql.lines().collect();
    let line_text = lines.get(position.line as usize).unwrap_or(&"");
    let text_before = &line_text[..position.character.min(line_text.len() as u32) as usize];

    cursor_identifier_token(text_before).contains('.')
}

pub(crate) fn predicate_operator_expected(sql: &str, position: Position) -> bool {
    let text_before = text_before_position(sql, position);
    if text_before.is_empty() {
        return false;
    }

    if !text_before
        .chars()
        .last()
        .is_some_and(|ch| ch.is_whitespace())
    {
        return false;
    }

    let trimmed = text_before.trim_end();
    let Some(last_char) = trimmed.chars().last() else {
        return false;
    };
    if matches!(
        last_char,
        '=' | '<' | '>' | '!' | '+' | '-' | '*' | '/' | '%' | '(' | ',' | '.'
    ) {
        return false;
    }

    let last_token = trimmed
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '(' | ')' | ',' | ';'))
        .rfind(|token| !token.is_empty())
        .unwrap_or("")
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`' | '[' | ']'));
    let last_token_upper = last_token.to_ascii_uppercase();

    !matches!(
        last_token_upper.as_str(),
        "WHERE"
            | "ON"
            | "HAVING"
            | "AND"
            | "OR"
            | "NOT"
            | "SET"
            | "UPDATE"
            | "WHEN"
            | "THEN"
            | "ELSE"
            | "LIKE"
            | "ILIKE"
            | "IN"
            | "BETWEEN"
            | "IS"
    )
}

pub(crate) fn latest_predicate_clause(sql: &str, position: Position) -> Option<&'static str> {
    let text_before = text_before_position(sql, position).to_ascii_uppercase();
    let statement_start = SqlParser::active_statement_start(&text_before);
    let statement = &text_before[statement_start..];

    ["WHERE", "HAVING", "ON", "WHEN", "SET", "UPDATE"]
        .into_iter()
        .filter_map(|clause| {
            previous_keyword_position(statement, clause).map(|position| (position, clause))
        })
        .max_by_key(|(position, _)| *position)
        .map(|(_, clause)| clause)
}

pub(crate) fn order_direction_keywords(
    sql: &str,
    position: Position,
    supports_nulls_ordering: bool,
    continuation_keywords: &[&'static str],
) -> Vec<&'static str> {
    let text_before = text_before_position(sql, position);
    let text_upper = text_before.to_ascii_uppercase();
    let statement_start = SqlParser::active_statement_start(text_before);
    let statement_upper = &text_upper[statement_start..];

    let Some(order_position) = statement_upper.rfind("ORDER BY") else {
        return Vec::new();
    };

    let after_order_start = statement_start + order_position + "ORDER BY".len();
    let after_order = text_before.get(after_order_start..).unwrap_or("");
    let segment = after_order.rsplit(',').next().unwrap_or(after_order);
    let trimmed = segment.trim_end();
    if trimmed.is_empty() || trimmed.ends_with('.') || trimmed.ends_with('(') {
        return Vec::new();
    }

    let has_trailing_whitespace = segment.chars().last().is_some_and(|ch| ch.is_whitespace());
    let raw_tokens = trimmed
        .split(|ch: char| ch.is_whitespace())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if raw_tokens.is_empty() {
        return Vec::new();
    }

    let complete_tokens = if has_trailing_whitespace {
        raw_tokens.as_slice()
    } else {
        &raw_tokens[..raw_tokens.len().saturating_sub(1)]
    };
    let complete_upper = complete_tokens
        .iter()
        .map(|token| normalize_order_token(token))
        .collect::<Vec<_>>();
    let last_complete = complete_upper.last().map(String::as_str);
    let previous_complete = complete_upper.iter().rev().nth(1).map(String::as_str);

    let mut keywords = match (previous_complete, last_complete) {
        (_, Some("NULLS")) if supports_nulls_ordering => vec!["FIRST", "LAST"],
        (Some("NULLS"), Some("FIRST" | "LAST")) => continuation_keywords.to_vec(),
        (_, Some("ASC" | "DESC")) => {
            let mut options = Vec::new();
            if supports_nulls_ordering {
                options.extend(["NULLS FIRST", "NULLS LAST"]);
            }
            options.extend_from_slice(continuation_keywords);
            options
        }
        _ => {
            let mut options = vec!["ASC", "DESC"];
            if supports_nulls_ordering {
                options.extend(["NULLS FIRST", "NULLS LAST"]);
            }
            options.extend_from_slice(continuation_keywords);
            options
        }
    };
    keywords.dedup();
    keywords
}

pub(crate) fn order_direction_sort_prefix(keyword: &str) -> &'static str {
    match keyword {
        "ASC" | "DESC" | "NULLS FIRST" | "NULLS LAST" | "FIRST" | "LAST" => "0",
        "," => "1",
        _ => "2",
    }
}

pub(crate) fn group_by_continuation_sort_prefix(keyword: &str) -> &'static str {
    match keyword {
        "," => "0",
        "HAVING" => "1",
        "ORDER BY" => "2",
        _ => "3",
    }
}

pub(crate) fn select_continuation_keywords(sql: &str, position: Position) -> Vec<&'static str> {
    let text_before = text_before_position(sql, position);
    let text_upper = text_before.to_ascii_uppercase();
    let statement_start = SqlParser::active_statement_start(text_before);
    let statement_upper = &text_upper[statement_start..];

    let Some(select_position) = statement_upper.rfind("SELECT") else {
        return vec![",", "AS", "FROM"];
    };

    let after_select_start = statement_start + select_position + "SELECT".len();
    let after_select = text_before.get(after_select_start..).unwrap_or("");
    let segment = after_select.rsplit(',').next().unwrap_or(after_select);
    let select_item = segment.trim();

    if select_item == "*" || select_item.ends_with(".*") {
        vec![",", "FROM"]
    } else {
        vec![",", "AS", "FROM"]
    }
}

pub(crate) fn expression_value_allows_default(sql: &str, position: Position) -> bool {
    let text_before = text_before_position(sql, position).to_ascii_uppercase();
    let statement_start = SqlParser::active_statement_start(&text_before);
    let statement = &text_before[statement_start..];

    if let Some(set_position) = insert_set_position(statement) {
        return !statement[set_position + "SET".len()..].contains("ON DUPLICATE");
    }

    if let Some(duplicate_update_position) = statement.rfind("ON DUPLICATE KEY UPDATE") {
        let after_duplicate_update = duplicate_update_position + "ON DUPLICATE KEY UPDATE".len();
        return !statement[after_duplicate_update..].contains("WHERE");
    }

    let Some(update_position) = statement.rfind("UPDATE") else {
        return false;
    };
    let Some(set_position) = statement.rfind("SET") else {
        return false;
    };
    if set_position < update_position {
        return false;
    }

    !statement[set_position + "SET".len()..].contains("WHERE")
}

pub(crate) fn predicate_continuation_keywords(
    sql: &str,
    position: Position,
    supports_returning: bool,
) -> Vec<&'static str> {
    let text_before = text_before_position(sql, position).to_ascii_uppercase();
    let statement_start = SqlParser::active_statement_start(&text_before);
    let statement = &text_before[statement_start..];

    let latest_clause = ["WHERE", "HAVING", "ON", "WHEN", "SET", "UPDATE"]
        .into_iter()
        .filter_map(|clause| {
            previous_keyword_position(statement, clause).map(|position| (position, clause))
        })
        .max_by_key(|(position, _)| *position);

    match latest_clause {
        Some((clause_position, clause))
            if between_first_value_needs_and(statement, clause_position, clause) =>
        {
            vec!["AND"]
        }
        Some((set_position, "SET")) if insert_set_position(statement) == Some(set_position) => {
            vec![",", "ON DUPLICATE KEY UPDATE"]
        }
        Some((_, "SET")) if supports_returning => vec![",", "WHERE", "RETURNING"],
        Some((_, "SET")) => vec![",", "WHERE"],
        Some((_, "UPDATE")) => vec![","],
        Some((_, "HAVING")) => vec!["AND", "OR", "ORDER BY", "LIMIT"],
        Some((_, "ON")) => vec!["AND", "OR", "WHERE", "GROUP BY", "ORDER BY", "LIMIT"],
        Some((_, "WHEN")) => vec!["AND", "OR", "THEN"],
        Some((_, "WHERE")) => vec!["AND", "OR", "GROUP BY", "HAVING", "ORDER BY", "LIMIT"],
        _ => vec!["AND", "OR"],
    }
}

fn insert_set_position(statement_upper: &str) -> Option<usize> {
    let insert_position = previous_keyword_position(statement_upper, "INSERT INTO")?;
    let set_position = previous_keyword_position(statement_upper, "SET")?;
    if set_position < insert_position {
        return None;
    }

    let after_insert = insert_position + "INSERT INTO".len();
    let between_insert_and_set = statement_upper.get(after_insert..set_position)?;
    if ["VALUES", "VALUE", "SELECT"]
        .into_iter()
        .any(|keyword| previous_keyword_position(between_insert_and_set, keyword).is_some())
    {
        return None;
    }

    Some(set_position)
}

pub(crate) fn case_continuation_keywords(sql: &str, position: Position) -> Vec<&'static str> {
    let text_before = text_before_position(sql, position);
    let raw_text_upper = text_before.to_ascii_uppercase();
    let searchable_text_upper = SqlParser::mask_sql_noise(&raw_text_upper);
    let statement_start = SqlParser::active_statement_start(&raw_text_upper);
    let searchable_statement = &searchable_text_upper[statement_start..];

    let Some(case_position) = previous_keyword_position(searchable_statement, "CASE") else {
        return vec!["WHEN", "ELSE", "END"];
    };
    let after_case = case_position + "CASE".len();
    if previous_keyword_position(&searchable_statement[after_case..], "END").is_some() {
        return Vec::new();
    }

    let latest_marker = ["THEN", "ELSE"]
        .into_iter()
        .filter_map(|marker| {
            previous_keyword_position(&searchable_statement[after_case..], marker)
                .map(|position| (position, marker))
        })
        .max_by_key(|(position, _)| *position)
        .map(|(_, marker)| marker);

    match latest_marker {
        Some("ELSE") => vec!["END"],
        Some("THEN") => vec!["WHEN", "ELSE", "END"],
        _ => vec!["WHEN", "ELSE", "END"],
    }
}

fn between_first_value_needs_and(
    statement_upper: &str,
    clause_position: usize,
    clause: &str,
) -> bool {
    if !matches!(clause, "WHERE" | "HAVING" | "ON" | "WHEN") {
        return false;
    }

    let after_clause = clause_position + clause.len();
    let Some(segment) = statement_upper.get(after_clause..) else {
        return false;
    };
    let Some(between_position) = previous_keyword_position(segment, "BETWEEN") else {
        return false;
    };
    let after_between = between_position + "BETWEEN".len();

    previous_keyword_position(&segment[after_between..], "AND").is_none()
}

fn previous_keyword_position(source_upper: &str, keyword: &str) -> Option<usize> {
    let mut search_pos = 0;
    let mut previous = None;

    while let Some(relative_position) = source_upper[search_pos..].find(keyword) {
        let position = search_pos + relative_position;
        if is_keyword_at(source_upper, position, keyword) {
            previous = Some(position);
        }
        search_pos = position + keyword.len();
    }

    previous
}

fn is_keyword_at(source_upper: &str, start: usize, keyword: &str) -> bool {
    let end = start + keyword.len();
    if end > source_upper.len() {
        return false;
    }

    let before_is_boundary = if start == 0 {
        true
    } else {
        source_upper[..start]
            .chars()
            .next_back()
            .is_none_or(|ch| !is_identifier_char(ch))
    };
    let after_is_boundary = if end == source_upper.len() {
        true
    } else {
        source_upper[end..]
            .chars()
            .next()
            .is_none_or(|ch| !is_identifier_char(ch))
    };

    before_is_boundary && after_is_boundary
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$')
}

fn normalize_order_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '[' | ']' | '(' | ')' | ';'))
        .to_ascii_uppercase()
}

pub(crate) fn cursor_identifier_token(text_before: &str) -> &str {
    let mut token_start = 0;
    let mut quote_end: Option<char> = None;
    let mut chars = text_before.char_indices().peekable();

    while let Some((index, ch)) = chars.next() {
        if let Some(end_quote) = quote_end {
            if ch == end_quote {
                if chars.peek().is_some_and(|(_, next)| *next == end_quote) {
                    chars.next();
                    continue;
                }
                quote_end = None;
            }
            continue;
        }

        match ch {
            '"' | '\'' | '`' => quote_end = Some(ch),
            '[' => quote_end = Some(']'),
            _ if ch.is_whitespace() || ch == ',' || ch == '(' => {
                token_start = index + ch.len_utf8();
            }
            _ => {}
        }
    }

    &text_before[token_start..]
}

pub(crate) fn text_before_position(sql: &str, position: Position) -> &str {
    let lines: Vec<&str> = sql.lines().collect();
    let mut offset = 0usize;

    for (line_index, line_text) in lines.iter().enumerate() {
        if line_index < position.line as usize {
            offset += line_text.len() + 1;
        } else if line_index == position.line as usize {
            offset += position.character.min(line_text.len() as u32) as usize;
            break;
        }
    }

    sql.get(..offset.min(sql.len())).unwrap_or(sql)
}
