use crate::{dialects::common, parser::SqlParser};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position};

#[derive(Default)]
struct Frame {
    from: bool,
    entry: Option<usize>,
}

/// Recognize only the unfinished introducer, never invocation arguments or aliases.
/// Full row/type inference belongs to the server's AST-backed local relations.
pub(super) fn entry_items(sql: &str, position: Position) -> Option<Vec<CompletionItem>> {
    if sql.len() > 512 * 1024 {
        return None;
    }
    let prefix = common::text_before_position(sql, position);
    if !prefix
        .as_bytes()
        .windows(7)
        .any(|s| s.eq_ignore_ascii_case(b"LATERAL"))
    {
        return None;
    }
    let mask = SqlParser::mask_sql_noise_for_dialect(prefix, "hive");
    let bytes = mask.as_bytes();
    let mut frames = vec![Frame::default()];
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        let frame = frames.last_mut()?;
        match bytes[index] {
            b'(' => {
                frame.entry = None;
                if frames.len() >= 64 {
                    return None;
                }
                frames.push(Frame::default());
            }
            b')' => {
                if frames.len() > 1 {
                    frames.pop();
                }
            }
            b';' => *frame = Frame::default(),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
                let word = &mask[start..index];
                if word.eq_ignore_ascii_case("SELECT") {
                    *frame = Frame::default();
                } else if word.eq_ignore_ascii_case("FROM") {
                    frame.from = true;
                } else if [
                    "WHERE",
                    "ON",
                    "USING",
                    "GROUP",
                    "ORDER",
                    "HAVING",
                    "LIMIT",
                    "UNION",
                    "EXCEPT",
                    "INTERSECT",
                ]
                .iter()
                .any(|s| word.eq_ignore_ascii_case(s))
                {
                    *frame = Frame::default();
                } else if word.eq_ignore_ascii_case("LATERAL")
                    && frame.from
                    && start > 0
                    && bytes[start - 1].is_ascii_whitespace()
                {
                    frame.entry = Some(index);
                }
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    let entry = frames.last()?.entry?;
    // Preserve punctuation and quoted input here; masking must not turn a string,
    // member name or comment into an empty function slot.
    let tail = prefix[entry..].to_ascii_lowercase();
    if !tail
        .chars()
        .all(|c| c.is_ascii_alphabetic() || c == '_' || c.is_whitespace())
    {
        return None;
    }
    let mut words: Vec<_> = tail.split_whitespace().collect();
    if tail.is_empty() || tail.ends_with(char::is_whitespace) {
        words.push("");
    }
    let functions = [
        "explode",
        "posexplode",
        "inline",
        "json_tuple",
        "parse_url_tuple",
        "stack",
    ];
    let (names, kind, prefix): (&[&str], _, _) = match words.as_slice() {
        [prefix] => (&["VIEW"], CompletionItemKind::KEYWORD, *prefix),
        ["view", prefix] => (functions.as_slice(), CompletionItemKind::FUNCTION, *prefix),
        ["view", "outer", prefix] => (functions.as_slice(), CompletionItemKind::FUNCTION, *prefix),
        _ => return None,
    };
    let mut items: Vec<_> = names
        .iter()
        .filter(|s| s.to_ascii_lowercase().starts_with(prefix))
        .map(|name| item(name, kind))
        .collect();
    if matches!(words.as_slice(), ["view", _]) && "outer".starts_with(prefix) {
        items.push(item("OUTER", CompletionItemKind::KEYWORD));
    }
    Some(items)
}

fn item(name: &str, kind: CompletionItemKind) -> CompletionItem {
    CompletionItem {
        label: name.into(),
        kind: Some(kind),
        insert_text: Some(name.into()),
        filter_text: Some(name.into()),
        sort_text: Some(format!("0_{name}")),
        detail: Some("Hive LATERAL VIEW".into()),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn entry_recognition_respects_query_clauses_noise_and_nesting_budgets() {
        for sql in [
            "SELECT 'LATERAL VIEW '",
            "SELECT * FROM t; SELECT LATERAL VIEW ",
            "SELECT * FROM t WHERE x = LATERAL VIEW ",
            "SELECT * FROM t WHERE EXISTS (SELECT LATERAL VIEW )",
            "SELECT * FROM t LATERAL VIEW explode(t.",
            "SELECT * FROM t LATERAL VIEW 'exp",
            "SELECT * FROM t LATERAL VIEW -- exp",
            "SELECT * FROM t LATERAL VIEW /* exp */ ",
            "SELECT t.LATERAL VIEW ",
        ] {
            assert!(
                entry_items(sql, Position::new(0, sql.len() as u32)).is_none(),
                "{sql}"
            );
        }
        let oversized = format!("{} SELECT * FROM t LATERAL VIEW ", " ".repeat(512 * 1024));
        assert!(entry_items(&oversized, Position::new(0, oversized.len() as u32)).is_none());
    }
}
