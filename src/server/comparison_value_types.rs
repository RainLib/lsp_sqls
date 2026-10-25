//! Expected comparison values reuse the projection/argument type owner. Only
//! types with a modeled value policy become positive completion knowledge.
use crate::{parser::SqlParser, schema::Schema};
use tower_lsp::lsp_types::{CompletionItem, Position, Range};
pub(super) struct ValueExpectation {
    pub(super) data_type: String,
    pub(super) role: Option<&'static str>,
}

pub(super) fn enrich(
    text: &str,
    position: Position,
    profile: &str,
    schema: Option<&Schema>,
    items: &mut Vec<CompletionItem>,
) -> Option<ValueExpectation> {
    let profile = super::completion_context::type_profile(profile)?;
    if text.len() > super::local_relations::LOCAL_RELATION_SCAN_MAX_BYTES
        || super::completion_regions::in_literal_or_comment(text, position, profile)
    {
        return None;
    }
    let cursor = super::completion_context::position_to_byte_offset(text, position);
    let before = &text[..cursor];
    let prefix_start = before
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_alphanumeric() && !matches!(ch, '_' | '$'))
        .map_or(0, |(at, ch)| at + ch.len_utf8());
    let prefix = &before[prefix_start..];
    if prefix.len() > 1024
        || prefix
            .chars()
            .next()
            .is_some_and(|ch| !ch.is_alphabetic() && ch != '_')
    {
        return None;
    }
    let mut suffix_end = cursor;
    for (offset, ch) in text[cursor..].char_indices() {
        if !ch.is_alphanumeric() && !matches!(ch, '_' | '$') {
            break;
        }
        suffix_end = cursor + offset + ch.len_utf8();
        if suffix_end - prefix_start > 1024 {
            return None;
        }
    }
    let range = Range::new(
        Position::new(
            position.line,
            position
                .character
                .checked_sub(prefix.encode_utf16().count() as u32)?,
        ),
        Position::new(
            position.line,
            position
                .character
                .checked_add(text[cursor..suffix_end].encode_utf16().count() as u32)?,
        ),
    );
    let mask = SqlParser::mask_sql_operand_context_for_dialect(&before[..prefix_start], profile);
    let tail = mask.trim_end();
    let similar = tail
        .get(tail.len().saturating_sub(2)..)
        .filter(|word| word.eq_ignore_ascii_case("TO"))
        .and_then(|_| {
            let before = tail.get(..tail.len().checked_sub(2)?)?;
            if before
                .chars()
                .next_back()
                .is_none_or(|ch| !ch.is_whitespace())
            {
                return None;
            }
            let previous = before.trim_end();
            let start = previous.len().checked_sub(7)?;
            (previous.get(start..)?.eq_ignore_ascii_case("SIMILAR")
                && previous[..start]
                    .chars()
                    .next_back()
                    .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_'))
            .then_some(start..tail.len())
        });
    let operator = similar.or_else(|| {
        [
            "<=>", "<=", ">=", "!=", "<>", "=", "<", ">", "!~*", "~*", "!~", "~", "ILIKE", "LIKE",
            "REGEXP", "RLIKE", "GLOB", "ESCAPE",
        ]
        .into_iter()
        .find_map(|operator| {
            let start = tail.len().checked_sub(operator.len())?;
            if !tail.get(start..)?.eq_ignore_ascii_case(operator) {
                return None;
            }
            let previous = tail[..start].chars().next_back();
            if operator.as_bytes()[0].is_ascii_alphabetic() {
                if previous.is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$')) {
                    return None;
                }
            } else if previous
                .is_some_and(|ch| matches!(ch, '=' | '<' | '>' | '!' | '~' | ':' | '|'))
            {
                return None;
            }
            Some(start..tail.len())
        })
    })?;
    // Supply a real RHS in a private inference source. Every binding and range
    // belongs to that same source; no recovered edit/range escapes to the UI.
    let probe = format!("{} NULL{}", &text[..operator.end], &text[cursor..]);
    // Build local bindings only for a proven value slot. This path has no
    // catalog/project manager and never registers a synchronized schema.
    let local;
    let schema = match schema {
        Some(schema) => schema,
        None => {
            local = super::document_semantics::local_schema(text, position, "", profile);
            &local
        }
    };
    let data_type = super::local_relations::infer_comparison_value_type(
        &probe,
        operator.clone(),
        schema,
        "",
        operator.end + 5,
        profile,
        None,
    )?;
    let escape = text[operator.clone()].eq_ignore_ascii_case("ESCAPE");
    if escape {
        crate::dialects::common::add_pattern_escape_value_items(items, prefix, range);
    } else {
        crate::dialects::common::add_expression_value_items(
            items, &data_type, profile, prefix, range,
        )?;
    }
    Some(ValueExpectation {
        data_type,
        role: escape.then_some("pattern-escape"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escape_character_contracts_are_structural_and_replace_full_original_suffixes() {
        use tower_lsp::lsp_types::{CompletionItemKind, CompletionTextEdit};
        for (profile, expected) in [
            ("postgres", "text"),
            ("duckdb", "varchar"),
            ("mysql", "varchar"),
            ("sqlite", "text"),
            ("sqlserver", "nvarchar"),
        ] {
            let prefix = "/*😀*/\r\nSELECT 1 WHERE 'x' LIKE 'x' ESCAPE ";
            let text = format!("{prefix}Suffix尾;");
            let mut items = vec![
                CompletionItem {
                    label: "TRUE".into(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    ..CompletionItem::default()
                },
                CompletionItem {
                    label: "''".into(),
                    kind: Some(CompletionItemKind::SNIPPET),
                    ..CompletionItem::default()
                },
            ];
            let value = enrich(
                &text,
                crate::position::lsp_position_at_end(prefix),
                profile,
                None,
                &mut items,
            )
            .unwrap();
            assert_eq!(value.data_type, expected);
            assert_eq!(value.role, Some("pattern-escape"));
            assert!(!items
                .iter()
                .any(|item| item.label == "TRUE" || item.label == "''" || item.label == "0"));
            let item = items.iter().find(|item| item.label == "'#'").unwrap();
            let Some(CompletionTextEdit::Edit(edit)) = item.text_edit.as_ref() else {
                panic!("escape values need edits")
            };
            assert_eq!(
                edit.range.start,
                crate::position::lsp_position_at_end(prefix)
            );
            assert_eq!(
                edit.range.end,
                crate::position::lsp_position_at_end(&format!("{prefix}Suffix尾"))
            );
            assert!(enrich(
                &(prefix.to_owned() + "'#' "),
                crate::position::lsp_position_at_end(&(prefix.to_owned() + "'#' ")),
                profile,
                None,
                &mut vec![]
            )
            .is_none());
        }
        for (profile, text) in [
            ("duckdb", "SELECT 1 WHERE 'x' SIMILAR TO 'x' ESCAPE "),
            ("hive", "SELECT 1 WHERE 'x' LIKE 'x' ESCAPE "),
            ("sqlite", "SELECT 1 WHERE 'x' GLOB 'x' ESCAPE "),
            ("postgres", "SELECT escape FROM users WHERE escape "),
        ] {
            assert!(
                enrich(
                    text,
                    crate::position::lsp_position_at_end(text),
                    profile,
                    None,
                    &mut vec![]
                )
                .is_none(),
                "{profile}: {text}"
            );
        }
    }
    #[test]
    fn typed_value_edits_consume_full_utf16_suffixes_and_keep_other_candidates() {
        use tower_lsp::lsp_types::{CompletionItemKind, CompletionTextEdit};
        for (profile, expression, partial, label) in [
            ("postgres", "CAST(TRUE AS BOOLEAN)", "T", "TRUE"),
            ("duckdb", "regexp_matches('oxide','^ox')", "", "TRUE"),
            ("sqlite", "CAST(1 AS INTEGER)", "", "0"),
            ("postgres", "LOWER('oxide')", "", "''"),
        ] {
            let source = format!("/*😀*/\r\nSELECT 1 WHERE {expression} = ");
            let before = format!("{source}{partial}");
            let text = format!("{before}Suffix尾;\r\nSELECT 2");
            let position = crate::position::lsp_position_at_end(&before);
            let mut items = vec![
                CompletionItem {
                    label: label.into(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    ..CompletionItem::default()
                },
                CompletionItem {
                    label: label.into(),
                    kind: Some(CompletionItemKind::FIELD),
                    ..CompletionItem::default()
                },
            ];
            assert!(enrich(&text, position, profile, None, &mut items).is_some());
            let values = items
                .iter()
                .filter(|item| item.label == label && item.kind != Some(CompletionItemKind::FIELD))
                .collect::<Vec<_>>();
            assert_eq!(values.len(), 1, "{profile}: {items:?}");
            let Some(CompletionTextEdit::Edit(edit)) = values[0].text_edit.as_ref() else {
                panic!("typed values need a full replacement edit: {profile}")
            };
            assert_eq!(
                edit.range.start,
                crate::position::lsp_position_at_end(&source)
            );
            assert_eq!(
                edit.range.end,
                crate::position::lsp_position_at_end(&format!("{before}Suffix尾"))
            );
            assert!(items.iter().any(|item| item.label == label
                && item.kind == Some(CompletionItemKind::FIELD)
                && item.text_edit.is_none()));
        }
    }
    #[test]
    fn pattern_operands_use_text_types_and_preserve_compound_operator_sources() {
        for (profile, expression, operator, expected) in [
            ("postgres", "LOWER('x')", "ILIKE", "text"),
            ("postgres", "'x'", "~*", "text"),
            ("duckdb", "'x'", "NOT SIMILAR /*😀*/ TO", "varchar"),
            ("duckdb", "'x'", "GLOB", "varchar"),
            ("mysql", "CAST(1 AS INTEGER)", "REGEXP", "varchar"),
            ("hive", "'x'", "RLIKE", "string"),
            (
                "clickhouse",
                "CAST(NULL AS Nullable(String))",
                "REGEXP",
                "String",
            ),
            ("sqlite", "CAST(1 AS INTEGER)", "NOT GLOB", "text"),
        ] {
            let text = format!("/*😀*/ SELECT 1 WHERE {expression} {operator} ");
            let mut items = vec![];
            assert_eq!(
                enrich(
                    &text,
                    crate::position::lsp_position_at_end(&text),
                    profile,
                    None,
                    &mut items
                )
                .as_ref()
                .map(|value| value.data_type.as_str()),
                Some(expected),
                "{text}"
            );
            assert!(items.iter().any(|item| item.label == "''"), "{text}");
            assert!(
                !items
                    .iter()
                    .any(|item| item.label == "0" || item.label == "TRUE"),
                "{text}"
            );
        }
        for (profile, text) in [
            ("sqlite", "SELECT 1 WHERE 'x' REGEXP "),
            ("duckdb", "SELECT 1 WHERE 'x' NOT GLOB "),
            ("postgres", "SELECT 1 WHERE ~ "),
        ] {
            assert!(
                enrich(
                    text,
                    crate::position::lsp_position_at_end(text),
                    profile,
                    None,
                    &mut vec![]
                )
                .is_none(),
                "{text}"
            );
        }
    }
    #[test]
    fn comparison_values_reuse_complex_expression_types() {
        let schema =
            super::super::document_semantics::local_schema("", Position::new(0, 0), "", "postgres");
        for (expression, expected) in [
            ("STRPOS('ab','b')", "integer"),
            ("CASE WHEN TRUE THEN 1 ELSE 2 END", "integer"),
            ("COALESCE(CAST(NULL AS integer), 3)", "integer"),
            ("LOWER('x')", "text"),
        ] {
            for operator in ["=", "<>", "<=", ">=", "<", ">"] {
                let text = format!("/*😀*/ SELECT 1 WHERE {expression} {operator} ");
                let mut items = vec![];
                let result = enrich(
                    &text,
                    crate::position::lsp_position_at_end(&text),
                    "postgres",
                    Some(&schema),
                    &mut items,
                );
                assert_eq!(
                    result.as_ref().map(|value| value.data_type.as_str()),
                    Some(expected),
                    "{text}"
                );
                assert!(
                    items
                        .iter()
                        .any(|item| item.label == if expected == "text" { "''" } else { "0" }),
                    "{text}"
                );
            }
        }
    }
    #[test]
    fn comments_opaque_regions_and_noncomparison_slots_do_not_invent_types() {
        let schema =
            super::super::document_semantics::local_schema("", Position::new(0, 0), "", "postgres");
        for text in [
            "SELECT 1 WHERE unknown_fn() = ",
            "SELECT 1 WHERE 1 + ",
            "SELECT 1 WHERE '1 = ",
            "SELECT 1 WHERE 1 -- = ",
            "SELECT 1 WHERE 1 = 2 ",
            "SELECT 1 WHERE 1 => ",
        ] {
            assert!(
                enrich(
                    text,
                    crate::position::lsp_position_at_end(text),
                    "postgres",
                    Some(&schema),
                    &mut vec![]
                )
                .is_none(),
                "{text}"
            );
        }
    }
    #[test]
    fn value_prefixes_case_and_literal_kinds_are_preserved_with_bounded_inference() {
        let schema =
            super::super::document_semantics::local_schema("", Position::new(0, 0), "", "postgres");
        for (expression, prefix, label) in [
            ("CAST(TRUE AS boolean)", "T", "TRUE"),
            ("CAST(1 AS bit(3))", "", "B'001'"),
            ("CAST(NULL AS integer[])", "", "NULL"),
            ("LOWER('x')", "", "''"),
        ] {
            let text = format!("SELECT 1 WHERE {expression} = {prefix}");
            let mut items = vec![];
            assert!(
                enrich(
                    &text,
                    crate::position::lsp_position_at_end(&text),
                    "postgres",
                    Some(&schema),
                    &mut items
                )
                .is_some(),
                "{text}"
            );
            assert!(
                items.iter().any(|item| item.label == label),
                "{text}: {items:?}"
            );
        }
        let text = format!(
            "{}SELECT 1 WHERE 1 = ",
            " ".repeat(super::super::local_relations::LOCAL_RELATION_SCAN_MAX_BYTES)
        );
        assert!(enrich(
            &text,
            crate::position::lsp_position_at_end(&text),
            "postgres",
            Some(&schema),
            &mut vec![]
        )
        .is_none());
    }
}
