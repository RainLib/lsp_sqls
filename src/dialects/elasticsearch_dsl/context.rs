use tower_lsp::lsp_types::Position;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EsCompletionKind {
    Broad,
    TopLevel,
    QueryType,
    AggType,
    BoolField,
    FieldName,
    IndexValue,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct EsCompletionContext {
    pub(super) kind: EsCompletionKind,
    pub(super) inside_string: bool,
}

#[derive(Debug, Clone, Default)]
struct EsJsonObjectFrame {
    owner_key: Option<String>,
    last_key: Option<String>,
    after_colon: bool,
}

#[derive(Debug, Clone, Default)]
struct EsJsonScanState {
    frames: Vec<EsJsonObjectFrame>,
    array_owner_keys: Vec<Option<String>>,
    pending_array_object_owner: Option<String>,
    previous_significant: Option<char>,
}

pub(super) fn elasticsearch_completion_context(
    source: &str,
    position: Position,
) -> EsCompletionContext {
    let byte_offset = byte_offset_at_position(source, position);
    let before = &source[..byte_offset.min(source.len())];
    let open_string_start = current_open_string_start(before);
    let context_source = open_string_start
        .map(|start| &before[..start])
        .unwrap_or(before);
    let state = scan_json_context(context_source);
    let inside_string = open_string_start.is_some();
    let previous = state.previous_significant;
    let current_key = state
        .frames
        .last()
        .and_then(|frame| frame.last_key.as_deref());
    let owner_key = state
        .frames
        .last()
        .and_then(|frame| frame.owner_key.as_deref());

    let kind = if matches!(previous, Some(':')) {
        match current_key {
            Some(key) if is_elasticsearch_index_key(key) => EsCompletionKind::IndexValue,
            Some(key) if is_elasticsearch_field_value_key(key) => EsCompletionKind::FieldName,
            _ => EsCompletionKind::Broad,
        }
    } else if is_json_key_position(previous) {
        match owner_key {
            None => EsCompletionKind::TopLevel,
            Some("query") => EsCompletionKind::QueryType,
            Some("aggs" | "aggregations") => EsCompletionKind::AggType,
            Some("bool") => EsCompletionKind::BoolField,
            Some(key) if is_elasticsearch_field_object_key(key) => EsCompletionKind::FieldName,
            _ => EsCompletionKind::Broad,
        }
    } else {
        EsCompletionKind::Broad
    };

    EsCompletionContext {
        kind,
        inside_string,
    }
}

fn byte_offset_at_position(source: &str, position: Position) -> usize {
    let position = crate::position::lsp_position_to_byte_position(source, position);
    let mut offset = 0usize;

    for (line_index, line) in source.split('\n').enumerate() {
        if line_index == position.line as usize {
            return offset + (position.character as usize).min(line.len());
        }
        offset += line.len() + 1;
    }

    source.len()
}

fn current_open_string_start(source: &str) -> Option<usize> {
    let mut in_string = false;
    let mut escaping = false;
    let mut start = 0usize;

    for (index, ch) in source.char_indices() {
        if in_string {
            if escaping {
                escaping = false;
            } else if ch == '\\' {
                escaping = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
            start = index;
        }
    }

    in_string.then_some(start)
}

fn scan_json_context(source: &str) -> EsJsonScanState {
    let mut state = EsJsonScanState::default();
    let mut in_string = false;
    let mut escaping = false;
    let mut string_start = 0usize;
    let mut previous_significant = None;

    for (index, ch) in source.char_indices() {
        if in_string {
            if escaping {
                escaping = false;
            } else if ch == '\\' {
                escaping = true;
            } else if ch == '"' {
                in_string = false;
                let value = source[string_start..index].to_string();
                if previous_significant != Some(':') {
                    if let Some(frame) = state.frames.last_mut() {
                        frame.last_key = Some(value);
                    }
                }
                previous_significant = Some('"');
            }
            continue;
        }

        if ch.is_whitespace() {
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                escaping = false;
                string_start = index + ch.len_utf8();
            }
            '{' => {
                let owner_key = if previous_significant == Some(':') {
                    state.frames.last().and_then(|frame| frame.last_key.clone())
                } else if previous_significant == Some('[') {
                    state.array_owner_keys.last().cloned().flatten()
                } else if previous_significant == Some(',') {
                    state.pending_array_object_owner.take()
                } else {
                    None
                };
                if let Some(frame) = state.frames.last_mut() {
                    if previous_significant == Some(':') {
                        frame.after_colon = false;
                    }
                }
                state.frames.push(EsJsonObjectFrame {
                    owner_key,
                    last_key: None,
                    after_colon: false,
                });
                previous_significant = Some('{');
            }
            '}' => {
                state.frames.pop();
                previous_significant = Some('}');
            }
            '[' => {
                let owner_key = if previous_significant == Some(':') {
                    state.frames.last().and_then(|frame| frame.last_key.clone())
                } else {
                    None
                };
                if let Some(frame) = state.frames.last_mut() {
                    if previous_significant == Some(':') {
                        frame.after_colon = false;
                    }
                }
                state.array_owner_keys.push(owner_key);
                state.pending_array_object_owner = None;
                previous_significant = Some('[');
            }
            ']' => {
                state.array_owner_keys.pop();
                state.pending_array_object_owner = None;
                previous_significant = Some(']');
            }
            ':' => {
                if let Some(frame) = state.frames.last_mut() {
                    frame.after_colon = true;
                }
                previous_significant = Some(':');
            }
            ',' => {
                let comma_in_array = matches!(previous_significant, Some('}' | ']'))
                    && !state.array_owner_keys.is_empty();
                if comma_in_array {
                    state.pending_array_object_owner =
                        state.array_owner_keys.last().cloned().flatten();
                } else if let Some(frame) = state.frames.last_mut() {
                    frame.last_key = None;
                    frame.after_colon = false;
                    state.pending_array_object_owner = None;
                }
                previous_significant = Some(',');
            }
            _ => {
                if let Some(frame) = state.frames.last_mut() {
                    if frame.after_colon {
                        frame.after_colon = false;
                    }
                }
                previous_significant = Some(ch);
            }
        }
    }

    state.previous_significant = previous_significant;
    state
}

fn is_json_key_position(previous: Option<char>) -> bool {
    matches!(previous, Some('{') | Some(',') | Some('"'))
}

fn is_elasticsearch_index_key(key: &str) -> bool {
    matches!(key, "index" | "_index")
}

fn is_elasticsearch_field_value_key(key: &str) -> bool {
    matches!(key, "field" | "fields")
}

fn is_elasticsearch_field_object_key(key: &str) -> bool {
    matches!(
        key,
        "match"
            | "match_phrase"
            | "match_phrase_prefix"
            | "term"
            | "terms"
            | "range"
            | "exists"
            | "prefix"
            | "wildcard"
            | "regexp"
            | "fuzzy"
            | "sort"
    )
}

pub(super) fn token_at_position(text: &str, position: Position) -> String {
    let position = crate::position::lsp_position_to_byte_position(text, position);
    let line = text.split('\n').nth(position.line as usize).unwrap_or("");
    let byte_index = position.character.min(line.len() as u32) as usize;
    let bytes = line.as_bytes();
    let mut start = byte_index.min(bytes.len());
    while start > 0 && is_token_char(bytes[start - 1] as char) {
        start -= 1;
    }
    let mut end = byte_index.min(bytes.len());
    while end < bytes.len() && is_token_char(bytes[end] as char) {
        end += 1;
    }

    line[start..end]
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

pub(super) fn is_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':')
}

pub(super) fn elasticsearch_index_at_position(source: &str, position: Position) -> Option<String> {
    let mut active_path = None;
    for (line_index, line) in source.lines().enumerate() {
        if line_index > position.line as usize {
            break;
        }
        let trimmed = line.trim();
        let Some((method, rest)) = trimmed.split_once(char::is_whitespace) else {
            continue;
        };
        if !matches!(
            method.to_ascii_uppercase().as_str(),
            "GET" | "POST" | "PUT" | "DELETE" | "HEAD" | "PATCH"
        ) {
            continue;
        }
        active_path = rest.split_whitespace().next().map(ToString::to_string);
    }

    let path = active_path?;
    let index = path
        .trim_matches(|character| character == '"' || character == '\'')
        .trim_start_matches('/')
        .split(['/', '?', '#'])
        .next()?
        .trim();
    if index.is_empty()
        || index.starts_with('_')
        || index
            .chars()
            .any(|character| matches!(character, ',' | '*' | '$' | '{' | '}'))
    {
        return None;
    }
    Some(index.to_string())
}
