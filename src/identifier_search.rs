//! Search metadata only after SQL name binding has selected visible inputs.
//! Character readings are immutable; comments always come from the live schema.
use crate::schema::Column;

mod pinyin_data;

const MAX_SEARCH_BYTES: usize = 2048;
const MAX_QUERY_BYTES: usize = 128;

fn han(ch: char) -> bool {
    matches!(ch as u32, 0x3007 | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af)
        || initial(ch).is_some()
}

fn initial(ch: char) -> Option<char> {
    let cp = ch as u32;
    let index = pinyin_data::PAGES
        .binary_search_by_key(&(cp >> 8), |(page, _)| *page)
        .ok()?;
    let byte = *pinyin_data::PAGES[index].1.get((cp & 255) as usize)?;
    (byte != b'.').then_some(byte as char)
}

fn initials(value: &str) -> Option<String> {
    if value.len() > MAX_SEARCH_BYTES || !value.chars().any(han) {
        return None;
    }
    let mut result = String::new();
    for ch in value.chars() {
        if let Some(first) = initial(ch) {
            result.push(first);
        } else if ch.is_ascii_alphanumeric() {
            result.push(ch.to_ascii_lowercase());
        } else if han(ch) {
            // Missing readings remain opaque, never invent a pronunciation.
            result.push(ch);
        }
    }
    Some(result)
}

pub(crate) fn pinyin_rank(value: &str, query: &str) -> Option<u8> {
    if query.is_empty()
        || query.len() > MAX_QUERY_BYTES
        || !query.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    let initials = initials(value)?;
    if initials.starts_with(query) {
        return Some(2);
    }
    let mut wanted = query.chars();
    let mut next = wanted.next();
    for ch in initials.chars() {
        if Some(ch) == next {
            next = wanted.next();
            if next.is_none() {
                return Some(3);
            }
        }
    }
    None
}

pub(crate) fn comment_prefix(comment: &str) -> &str {
    let mut end = comment.len().min(MAX_SEARCH_BYTES);
    while !comment.is_char_boundary(end) {
        end -= 1;
    }
    &comment[..end]
}

pub(crate) fn column_match_rank(column: &Column, query: &str) -> Option<u8> {
    metadata_match_rank(&column.name, column.comment.as_deref(), query)
}

fn metadata_match_rank(name: &str, comment: Option<&str>, query: &str) -> Option<u8> {
    if let Some(rank) = crate::dialects::common::identifier_match_rank(name, query) {
        return Some(rank);
    }
    if query.len() > MAX_QUERY_BYTES {
        return None;
    }
    let comment = comment_prefix(comment?);
    crate::dialects::common::identifier_match_rank(comment, query).map(|_| 4)
}

pub(crate) fn filter_text(name: &str, comment: Option<&str>) -> String {
    let mut text = name.to_owned();
    if let Some(initials) = initials(name) {
        text.push(' ');
        text.push_str(&initials);
    }
    if let Some(comment) = comment.map(comment_prefix).filter(|s| !s.is_empty()) {
        text.push(' ');
        text.push_str(comment);
        if let Some(initials) = initials(comment) {
            text.push(' ');
            text.push_str(&initials);
        }
    }
    text
}

pub(crate) fn table_data(name: &str) -> Option<serde_json::Value> {
    name.chars()
        .any(han)
        .then(|| serde_json::json!({"oxide":{"searchTable":name}}))
}

/// Keep exact identities/edits intact while making auxiliary matches survive
/// Monaco filtering. Scan all bound candidates before capping, so a late exact
/// name wins; no stale catalog index is retained between requests.
pub(crate) fn rank_items(
    items: &mut Vec<tower_lsp::lsp_types::CompletionItem>,
    query: &str,
) -> bool {
    if query.is_empty()
        || !items.iter().any(|item| {
            let Some(data) = item.data.as_ref().and_then(|d| d.get("oxide")) else {
                return false;
            };
            let Some(name) = data
                .get("column")
                .or_else(|| data.get("searchTable"))
                .and_then(serde_json::Value::as_str)
            else {
                return false;
            };
            let comment = data
                .get("searchComment")
                .and_then(serde_json::Value::as_str);
            // Plain rows keep their existing client-filtered contract. Rows
            // with searchable metadata are filtered on every fresh snapshot.
            name.chars().any(han) || comment.is_some_and(|s| !s.is_empty())
        })
    {
        return false;
    }
    items.retain_mut(|item| {
        let Some(data) = item.data.as_ref().and_then(|d| d.get("oxide")) else {
            return true;
        };
        let column = data.get("column").and_then(serde_json::Value::as_str);
        let table = data.get("searchTable").and_then(serde_json::Value::as_str);
        let Some(name) = column.or(table) else {
            return true;
        };
        let comment = data
            .get("searchComment")
            .and_then(serde_json::Value::as_str);
        let Some(rank) = metadata_match_rank(name, comment, query) else {
            return false;
        };
        let old = item.sort_text.as_deref().unwrap_or("2:");
        let (group, rest) = old.split_once(':').unwrap_or((old, ""));
        item.sort_text = Some(format!("{group}:{rank}:{rest}"));
        item.filter_text = Some(format!(
            "{query} {}",
            item.filter_text.as_deref().unwrap_or(name)
        ));
        true
    });
    items.sort_by(|a, b| a.sort_text.cmp(&b.sort_text));
    let mut metadata = 0;
    let mut incomplete = false;
    items.retain(|item| {
        let bound = item
            .data
            .as_ref()
            .and_then(|d| d.get("oxide"))
            .is_some_and(|d| d.get("column").is_some() || d.get("searchTable").is_some());
        if !bound {
            return true;
        }
        metadata += 1;
        if metadata > 256 {
            incomplete = true;
            false
        } else {
            true
        }
    });
    incomplete
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dictionary_pages_are_sorted_sized_and_only_contain_known_ascii_readings() {
        let mut previous = None;
        let mut count = 0;
        for (page, bytes) in pinyin_data::PAGES {
            assert!(previous.is_none_or(|p| p < *page));
            previous = Some(*page);
            assert_eq!(bytes.len(), 256);
            assert!(bytes.iter().all(|b| *b == b'.' || b.is_ascii_lowercase()));
            count += bytes.iter().filter(|b| **b != b'.').count();
            // Every generated reading must be reachable, including Han marks
            // outside the usual ideograph blocks (for example U+3005).
            for (offset, byte) in bytes.iter().enumerate().filter(|(_, b)| **b != b'.') {
                let ch = char::from_u32(page * 256 + offset as u32).unwrap();
                assert_eq!(
                    initials(&ch.to_string()),
                    Some((*byte as char).to_string()),
                    "U+{:X}",
                    ch as u32
                );
            }
        }
        assert_eq!(count, 21133);
        assert_eq!(initial('租'), Some('z'));
        assert_eq!(initial('𠀀'), None);
    }
    #[test]
    fn search_readings_match_default_character_readings_and_mixed_ascii() {
        assert_eq!(initials("中文VIP_金额2"), Some("zwvipje2".into()));
        assert_eq!(pinyin_rank("租金总额", "zj"), Some(2));
        assert_eq!(pinyin_rank("总租金", "zj"), Some(3));
        assert_eq!(pinyin_rank("总租金", "jz"), None);
        assert_eq!(pinyin_rank("𠀀", "he"), None);
    }
    #[test]
    fn search_budgets_bound_comments_and_extra_filter_payload_without_truncating_names() {
        let comment = format!("{}needle", "界".repeat(1024));
        let column = Column {
            name: "amount".into(),
            comment: Some(comment.clone()),
            ..Default::default()
        };
        assert_eq!(column_match_rank(&column, "needle"), None);
        assert_eq!(column_match_rank(&column, &"a".repeat(129)), None);
        assert!(filter_text("amount", Some(&comment)).len() < 4105);
        let name = "长".repeat(1024);
        assert!(filter_text(&name, None).starts_with(&name));
    }
}
