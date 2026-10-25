//! Positive logical-operand knowledge, separate from IS truth tests and RHS
//! value templates. One bounded AST probe drives both candidates and metadata.
use crate::{
    parser::{CompletionContext, SqlParser},
    schema::Schema,
};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, Position, Range, TextEdit,
};

#[derive(Clone, Copy)]
pub(super) struct LogicalContinuation {
    pub(super) logical: bool,
    pub(super) pattern_escape: Option<bool>,
}
pub(super) fn enrich(
    text: &str,
    position: Position,
    profile: &str,
    schema: Option<&Schema>,
    items: &mut Vec<CompletionItem>,
) -> Option<LogicalContinuation> {
    let profile = super::completion_context::type_profile(profile)?;
    if text.len() > super::local_relations::LOCAL_RELATION_SCAN_MAX_BYTES
        || super::completion_regions::in_literal_or_comment(text, position, profile)
    {
        return None;
    }
    let cursor = super::completion_context::position_to_byte_offset(text, position);
    let before = &text[..cursor];
    let start = if before
        .chars()
        .next_back()
        .is_some_and(|ch| ch.is_whitespace() || ch == ')')
    {
        cursor
    } else {
        before
            .char_indices()
            .rev()
            .find(|(_, ch)| !ch.is_alphanumeric() && !matches!(ch, '_' | '$'))
            .map_or(0, |(at, ch)| at + ch.len_utf8())
    };
    let prefix = before[start..].to_ascii_uppercase();
    if !prefix.is_empty()
        && (!["AND", "OR", "ESCAPE"]
            .iter()
            .any(|keyword| keyword.starts_with(&prefix))
            || before[..start]
                .chars()
                .next_back()
                .is_none_or(|ch| !ch.is_whitespace()))
    {
        return None;
    }
    let mut end = cursor;
    for (offset, ch) in text[cursor..].char_indices() {
        if !ch.is_alphanumeric() && !matches!(ch, '_' | '$') {
            break;
        }
        end = cursor + offset + ch.len_utf8();
        if end - start > 1024 {
            return None;
        }
    }
    if end == cursor && matches!(prefix.as_str(), "AND" | "OR" | "ESCAPE") {
        return None;
    }
    let source = &text[..start];
    let masked = SqlParser::mask_sql_operand_context_for_dialect(source, profile);
    let last = masked.trim_end().chars().next_back()?;
    if matches!(
        last,
        '=' | '<' | '>' | '!' | '+' | '-' | '*' | '/' | '%' | '(' | ',' | '.' | '|'
    ) {
        return None;
    }
    if !matches!(
        SqlParser::completion_context_from_text(
            &masked,
            crate::position::byte_position_at_end(&masked)
        ),
        CompletionContext::WhereClause
            | CompletionContext::HavingClause
            | CompletionContext::JoinConditionClause
            | CompletionContext::PredicateContinuationClause
    ) {
        return None;
    }
    let token = masked
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .rfind(|word| !word.is_empty())
        .unwrap_or("")
        .to_ascii_uppercase();
    if source
        .trim_end()
        .chars()
        .next_back()
        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        && [
            "WHERE", "ON", "HAVING", "WHEN", "AND", "OR", "NOT", "THEN", "ELSE", "IN", "IS",
            "BETWEEN", "LIKE", "ILIKE",
        ]
        .contains(&token.as_str())
    {
        return None;
    }
    // AND binds below a complete matching predicate. An appended equality
    // instead becomes part of its pattern operand and loses that predicate.
    let probe = format!("{source} AND NULL{}", &text[end..]);
    let local;
    let schema = match schema {
        Some(value) => value,
        None => {
            local = super::document_semantics::local_schema(text, position, "", profile);
            &local
        }
    };
    let knowledge = super::local_relations::infer_continuation_type(
        &probe,
        start + 1..start + 4,
        schema,
        start + 9,
        profile,
    );
    let allowed = knowledge.logical?;
    items.retain(|item| {
        !matches!(
            item.kind,
            Some(CompletionItemKind::KEYWORD | CompletionItemKind::OPERATOR)
        ) || !["AND", "OR", "ESCAPE"].contains(&item.label.to_ascii_uppercase().as_str())
    });
    if allowed {
        let range = Range {
            start: Position::new(position.line, position.character - (cursor - start) as u32),
            end: Position::new(
                position.line,
                position.character + text[cursor..end].encode_utf16().count() as u32,
            ),
        };
        for label in ["AND", "OR", "ESCAPE"]
            .into_iter()
            .filter(|label| *label != "ESCAPE" || knowledge.escape == Some(true))
            .filter(|label| label.starts_with(&prefix))
        {
            let value = format!(
                "{}{label} ",
                if source.chars().next_back().is_some_and(char::is_whitespace) {
                    ""
                } else {
                    " "
                }
            );
            items.push(CompletionItem {
                label: label.into(),
                kind: Some(CompletionItemKind::KEYWORD),
                insert_text: Some(value.clone()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: value,
                })),
                sort_text: Some(format!("0:logical:{label}")),
                ..CompletionItem::default()
            });
        }
    }
    Some(LogicalContinuation {
        logical: allowed,
        pattern_escape: allowed.then_some(knowledge.escape).flatten(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // Existing assertions exercise logical knowledge; capability tests below
    // inspect the complete result directly through the production function.
    fn enrich(
        text: &str,
        position: Position,
        profile: &str,
        schema: Option<&Schema>,
        items: &mut Vec<CompletionItem>,
    ) -> Option<bool> {
        super::enrich(text, position, profile, schema, items).map(|knowledge| knowledge.logical)
    }
    #[test]
    fn optional_escape_capability_distinguishes_supported_denied_and_unknown_versions() {
        for (profile, version, expression, expected) in [
            ("clickhouse", None, "'x' LIKE 'x'", None),
            ("clickhouse", Some("26.5.2"), "'x' LIKE 'x'", Some(false)),
            (
                "clickhouse",
                Some("26.6.1.123"),
                "'x' ILIKE 'x'",
                Some(true),
            ),
            (
                "clickhouse",
                Some("26.10.1"),
                "'x' SIMILAR TO 'x'",
                Some(true),
            ),
            ("postgres", None, "'x' LIKE 'x'", Some(true)),
            ("postgres", None, "'x' ~ 'x'", Some(false)),
            ("duckdb", None, "'x' SIMILAR TO 'x'", Some(false)),
            ("hive", None, "'x' LIKE 'x'", Some(false)),
            ("sqlserver", None, "'x' LIKE 'x' ESCAPE NULL", Some(false)),
        ] {
            let text = format!("SELECT 1 WHERE {expression} ");
            let position = crate::position::lsp_position_at_end(&text);
            let mut schema =
                super::super::document_semantics::local_schema(&text, position, "", profile);
            schema.server_version = version.map(str::to_owned);
            let knowledge =
                super::enrich(&text, position, profile, Some(&schema), &mut vec![]).unwrap();
            assert!(knowledge.logical, "{profile}: {expression}");
            assert_eq!(knowledge.pattern_escape, expected, "{profile}: {version:?}");
        }
    }
    #[test]
    fn nullable_and_dynamic_matching_operands_keep_the_complete_logical_predicate() {
        for expression in [
            "'x' LIKE 'x' ESCAPE SUBSTR('##',1,1)",
            "'x' LIKE 'x' ESCAPE NULL",
        ] {
            let source = format!("SELECT 1 WHERE {expression} ");
            let mut items = vec![];
            let actual = enrich(
                &source,
                crate::position::lsp_position_at_end(&source),
                "duckdb",
                None,
                &mut items,
            );
            assert_eq!(actual, Some(true), "{expression}");
        }
    }
    #[test]
    fn completed_matching_predicates_keep_logical_knowledge_after_literal_operands() {
        for (profile, expression) in [
            ("postgres", "'oxide' LIKE 'ox%'"),
            ("postgres", "'oxide_1' SIMILAR TO 'oxide#_1' ESCAPE '#'"),
            ("duckdb", "'oxide_1' ILIKE 'oxide#_1' ESCAPE '#'"),
            ("mysql", "'oxide' REGEXP '^ox'"),
            ("sqlite", "'oxide' NOT GLOB 'db*'"),
            ("hive", "'oxide' RLIKE '^ox'"),
            ("clickhouse", "'oxide' LIKE 'ox%'"),
            ("sqlserver", "'oxide_1' LIKE 'oxide#_1' ESCAPE '#'"),
            ("postgres", "CAST(TRUE AS boolean) AND 'oxide' LIKE 'ox%'"),
        ] {
            let text = format!("/*😀*/\r\nSELECT 1 WHERE {expression} ");
            let mut items = vec![];
            assert_eq!(
                enrich(
                    &text,
                    crate::position::lsp_position_at_end(&text),
                    profile,
                    None,
                    &mut items
                ),
                Some(true),
                "{profile}: {expression}"
            );
            assert!(
                items.iter().any(|item| item.label == "AND"),
                "{profile}: {expression}"
            );
        }
    }
    #[test]
    fn optional_escape_discovery_uses_the_matching_operand_and_full_utf16_edit() {
        for (profile, operator) in [
            ("postgres", "LIKE"),
            ("postgres", "NOT ILIKE"),
            ("postgres", "SIMILAR TO"),
            ("duckdb", "ILIKE"),
            ("mysql", "LIKE"),
            ("sqlite", "NOT LIKE"),
            ("sqlserver", "LIKE"),
        ] {
            let source =
                format!("/*😀*/\r\nSELECT 1 WHERE LOWER('oxide_1') {operator} 'oxide#_1' ");
            let prefix = format!("{source}E");
            let text = format!("{prefix}Suffix尾;");
            let mut items = vec![];
            enrich(
                &text,
                crate::position::lsp_position_at_end(&prefix),
                profile,
                None,
                &mut items,
            );
            let item = items
                .iter()
                .find(|item| item.label == "ESCAPE")
                .unwrap_or_else(|| panic!("{profile}: {operator}: {items:?}"));
            let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
                panic!("missing escape edit")
            };
            assert_eq!(
                edit.range.start,
                crate::position::lsp_position_at_end(&source)
            );
            assert_eq!(
                edit.range.end,
                crate::position::lsp_position_at_end(&text[..text.len() - 1])
            );
            assert_eq!(edit.new_text, "ESCAPE ");
        }
    }
    #[test]
    fn optional_escape_is_not_added_after_unrelated_wrapped_or_already_escaped_values() {
        for (profile, expression) in [
            ("postgres", "('x' LIKE 'x')"),
            ("postgres", "'x' LIKE 'x' ESCAPE '#'"),
            ("postgres", "'x' ~ '^x'"),
            ("duckdb", "'x' SIMILAR TO 'x'"),
            ("sqlite", "'x' GLOB 'x'"),
            ("mysql", "'x' REGEXP 'x'"),
            ("hive", "'x' LIKE 'x'"),
            ("clickhouse", "'x' LIKE 'x'"),
            ("postgres", "'x' LIKE"),
        ] {
            let text = format!("SELECT 1 WHERE {expression} ");
            let mut items = vec![];
            enrich(
                &text,
                crate::position::lsp_position_at_end(&text),
                profile,
                None,
                &mut items,
            );
            assert!(
                !items.iter().any(|item| item.label == "ESCAPE"),
                "{profile}: {expression}"
            );
        }
    }
    #[test]
    fn analytic_exists_predicates_have_typed_logical_continuations() {
        for profile in ["duckdb", "hive", "hql", "clickhouse", "ch"] {
            for expression in [
                "EXISTS(SELECT NULL)",
                "NOT EXISTS(SELECT NULL WHERE 1=0)",
                "EXISTS(SELECT 1) AND NOT EXISTS(SELECT 1 WHERE 1=0)",
                "CASE WHEN 1=1 THEN EXISTS(SELECT 1) ELSE EXISTS(SELECT 2) END",
            ] {
                let text = format!("/*😀*/ SELECT 1 WHERE {expression} ");
                let mut items = vec![];
                assert_eq!(
                    enrich(
                        &text,
                        crate::position::lsp_position_at_end(&text),
                        profile,
                        None,
                        &mut items
                    ),
                    Some(true),
                    "{profile}: {expression}"
                );
                assert!(
                    items.iter().any(|item| item.label == "AND"),
                    "{profile}: {expression}"
                );
            }
        }
    }
    #[test]
    fn scalar_logical_policies_are_explicit_and_dialect_specific() {
        assert_eq!(
            crate::dialects::common::type_supports_logical_continuation(
                "Decimal64(2)",
                "clickhouse"
            ),
            Some(false)
        );
        assert_eq!(
            crate::dialects::common::type_supports_logical_continuation(
                "enum('yes','no')",
                "mysql"
            ),
            Some(true)
        );
        assert_eq!(
            crate::dialects::common::type_supports_logical_continuation(
                "enum('true','false')",
                "duckdb"
            ),
            None
        );
        for (profile, expression, expected) in [
            ("postgres", "CAST(1 AS INTEGER)", false),
            ("postgres", "LOWER('x')", false),
            ("postgres", "CAST(TRUE AS boolean)", true),
            ("sqlite", "CAST(1 AS INTEGER)", true),
            ("mysql", "CAST(1 AS DECIMAL(8,2))", true),
            ("sqlserver", "CAST(1 AS BIT)", false),
            ("hive", "CAST(TRUE AS BOOLEAN)", true),
            ("clickhouse", "CAST(1 AS UInt8)", true),
            ("ch", "CAST(1 AS Decimal64(2))", false),
            ("duckdb", "CAST(1 AS INTEGER)", true),
        ] {
            let text = format!("/*😀*/ SELECT 1 WHERE {expression} ");
            let mut items = vec![];
            assert_eq!(
                enrich(
                    &text,
                    crate::position::lsp_position_at_end(&text),
                    profile,
                    None,
                    &mut items
                ),
                Some(expected),
                "{text} {profile}"
            );
            assert_eq!(items.iter().any(|item| item.label == "AND"), expected);
        }
    }
    #[test]
    fn unrelated_and_opaque_slots_do_not_acquire_logical_knowledge() {
        for text in [
            "SELECT LOWER('x') ",
            "SELECT * FROM users ",
            "SELECT 1 WHERE ",
            "SELECT 1 WHERE 1 = ",
            "SELECT 1 WHERE unknown_fn() ",
            "SELECT 1 WHERE 'unclosed ",
            "SELECT 1 WHERE 1 -- trailing ",
        ] {
            assert_eq!(
                enrich(
                    text,
                    crate::position::lsp_position_at_end(text),
                    "postgres",
                    None,
                    &mut vec![]
                ),
                None,
                "{text}"
            );
        }
    }
    #[test]
    fn logical_prefix_edits_replace_the_full_suffix_without_leaking_private_probe_offsets() {
        let prefix = "/*😀*/ SELECT 1 WHERE CAST(TRUE AS boolean) A";
        let text = format!("{prefix}Suffix;");
        let mut items = vec![CompletionItem {
            label: "OR".into(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..CompletionItem::default()
        }];
        assert_eq!(
            enrich(
                &text,
                crate::position::lsp_position_at_end(prefix),
                "postgres",
                None,
                &mut items
            ),
            Some(true)
        );
        assert!(items.iter().all(|item| item.label == "AND"));
        let Some(CompletionTextEdit::Edit(edit)) = items[0].text_edit.as_ref() else {
            panic!("missing keyword edit")
        };
        assert_eq!(
            edit.range.start.character,
            prefix.encode_utf16().count() as u32 - 1
        );
        assert_eq!(
            edit.range.end.character,
            text.encode_utf16().count() as u32 - 1
        );
        assert_eq!(edit.new_text, "AND ");
    }
    #[test]
    fn predicate_type_probes_remain_bounded_for_large_documents_and_replacement_words() {
        let prefix = "SELECT 1 WHERE CAST(TRUE AS boolean) A";
        for text in [
            format!("{prefix}{}", "x".repeat(1025)),
            format!(
                "{}SELECT 1 WHERE CAST(TRUE AS boolean) ",
                " ".repeat(super::super::local_relations::LOCAL_RELATION_SCAN_MAX_BYTES)
            ),
        ] {
            assert_eq!(
                enrich(
                    &text,
                    crate::position::lsp_position_at_end(prefix),
                    "postgres",
                    None,
                    &mut vec![]
                ),
                None
            );
        }
    }
}
