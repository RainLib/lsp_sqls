//! Completion case for the SQL language server.
use super::completion_context::{is_relation_completion_kind, next_snippet_tabstop_index};
use super::completion_identifiers::{
    relation_alias_context_at_position, table_alias_initials, update_completion_text_edit,
};
use super::preferences::{CompletionPreferences, KeywordCase, TableAliasStyle};
use tower_lsp::lsp_types::*;
pub(super) fn apply_completion_preferences(
    text: &str,
    position: Position,
    items: &mut [CompletionItem],
    preferences: &CompletionPreferences,
) {
    let add_table_alias = preferences.table_alias == TableAliasStyle::Initials
        && relation_alias_context_at_position(text, position);

    for item in items {
        if item.kind == Some(CompletionItemKind::KEYWORD) {
            let transform = |value: &str| apply_identifier_case(value, preferences.keyword_case);
            item.label = transform(&item.label);
            if let Some(insert_text) = item.insert_text.as_mut() {
                *insert_text = transform(insert_text);
            }
            update_completion_text_edit(item, &transform);
        } else if completion_item_uses_generated_keyword_case(item) {
            apply_generated_function_case(item, preferences.keyword_case);
        }

        if completion_item_uses_generated_function_case(item) {
            apply_generated_function_case(item, preferences.function_case);
            if item
                .sort_text
                .as_deref()
                .is_some_and(|sort_text| sort_text.starts_with("1:window-function:"))
            {
                apply_generated_template_keyword_case(item, preferences.keyword_case);
            }
        }

        if add_table_alias && is_relation_completion_kind(item.kind) {
            let Some(alias) = table_alias_initials(&item.label) else {
                continue;
            };
            let append_alias = |value: &str| {
                let tabstop = next_snippet_tabstop_index(value);
                format!("{value} ${{{tabstop}:{alias}}}")
            };
            if let Some(insert_text) = item.insert_text.as_mut() {
                *insert_text = append_alias(insert_text);
            } else {
                item.insert_text = Some(append_alias(&item.label));
            }
            item.insert_text_format = Some(InsertTextFormat::SNIPPET);
            update_completion_text_edit(item, &append_alias);
        }
    }
}

pub(super) fn apply_identifier_case(value: &str, requested_case: KeywordCase) -> String {
    match requested_case {
        KeywordCase::Upper => value.to_ascii_uppercase(),
        KeywordCase::Lower => value.to_ascii_lowercase(),
        KeywordCase::Preserve => value.to_string(),
    }
}

pub(super) fn completion_item_uses_generated_keyword_case(item: &CompletionItem) -> bool {
    let sort_text = item.sort_text.as_deref().unwrap_or_default();
    sort_text.starts_with("1:builtin-value:")
        || (sort_text.starts_with("1:oracle-table-function:")
            && matches!(item.label.to_ascii_uppercase().as_str(), "TABLE" | "THE"))
}

pub(super) fn completion_item_uses_generated_function_case(item: &CompletionItem) -> bool {
    let sort_text = item.sort_text.as_deref().unwrap_or_default();
    sort_text.starts_with("1:builtin:")
        || sort_text.starts_with("1:window-function:")
        || sort_text.starts_with("1:table-function:")
        || (sort_text.starts_with("1:oracle-table-function:")
            && !matches!(item.label.to_ascii_uppercase().as_str(), "TABLE" | "THE"))
}

pub(super) fn replace_identifier_prefix(value: &str, original: &str, replacement: &str) -> String {
    let trimmed = value.trim_start_matches(char::is_whitespace);
    let leading_len = value.len().saturating_sub(trimmed.len());
    let Some(prefix) = trimmed.get(..original.len()) else {
        return value.to_string();
    };
    if !prefix.eq_ignore_ascii_case(original) {
        return value.to_string();
    }
    format!(
        "{}{}{}",
        &value[..leading_len],
        replacement,
        &trimmed[original.len()..]
    )
}

pub(super) fn apply_generated_function_case(item: &mut CompletionItem, function_case: KeywordCase) {
    let original = item.label.clone();
    let replacement = apply_identifier_case(&original, function_case);
    item.label = replacement.clone();
    if item
        .filter_text
        .as_deref()
        .is_some_and(|filter_text| filter_text.eq_ignore_ascii_case(&original))
    {
        item.filter_text = Some(replacement.clone());
    }
    let transform = |value: &str| replace_identifier_prefix(value, &original, &replacement);
    if let Some(insert_text) = item.insert_text.as_mut() {
        *insert_text = transform(insert_text);
    }
    update_completion_text_edit(item, &transform);
}

pub(super) fn generated_template_keyword_case(value: &str, keyword_case: KeywordCase) -> String {
    if matches!(keyword_case, KeywordCase::Preserve) {
        return value.to_string();
    }
    const KEYWORDS: &[&str] = &["AS", "INTERVAL", "OVER", "PARTITION", "BY", "ORDER"];
    let characters = value.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0usize;
    while cursor < characters.len() {
        if !characters[cursor].is_ascii_alphanumeric() && characters[cursor] != '_' {
            output.push(characters[cursor]);
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < characters.len()
            && (characters[cursor].is_ascii_alphanumeric() || characters[cursor] == '_')
        {
            cursor += 1;
        }
        let word = characters[start..cursor].iter().collect::<String>();
        if KEYWORDS
            .iter()
            .any(|keyword| word.eq_ignore_ascii_case(keyword))
        {
            output.push_str(&apply_identifier_case(&word, keyword_case));
        } else {
            output.push_str(&word);
        }
    }
    output
}

pub(super) fn apply_generated_template_keyword_case(
    item: &mut CompletionItem,
    keyword_case: KeywordCase,
) {
    let transform = |value: &str| generated_template_keyword_case(value, keyword_case);
    if let Some(insert_text) = item.insert_text.as_mut() {
        *insert_text = transform(insert_text);
    }
    update_completion_text_edit(item, &transform);
}
