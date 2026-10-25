//! Complete the constrained keyword part of IS predicates. SQLite's ordinary
//! IS/IS NOT accepts arbitrary expressions, so its operand lists stay intact.
use super::completion_context::position_to_byte_offset;
use crate::parser::SqlParser;
use std::ops::Range;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, Position, TextEdit,
};

struct TailToken {
    word: String,
    span: Range<usize>,
}

pub(super) fn completion_candidates(
    text: &str,
    position: Position,
    profile: &str,
    source: impl FnOnce() -> Option<crate::schema::Schema>,
) -> Option<super::completion_context::CompletionCandidates> {
    let profile = match profile {
        "postgres" | "postgresql" | "pgsql" | "psql" => "postgres",
        "mysql" | "mariadb" => "mysql",
        "sqlserver" | "mssql" | "tsql" => "sqlserver",
        "sqlite" | "sqlite3" => "sqlite",
        "duckdb" => "duckdb",
        "hive" | "hql" => "hive",
        "clickhouse" => "clickhouse",
        _ => return None,
    };
    let cursor = position_to_byte_offset(text, position);
    if cursor > 524288 {
        return None;
    }
    if super::completion_regions::in_literal_or_comment(text, position, profile) {
        return Some(Vec::new().into());
    }
    let before = text.get(..cursor)?;
    let mask = SqlParser::mask_sql_operand_context_for_dialect(before, profile);
    let tokens = tail_tokens(&mask, before)?;
    let at = tokens.iter().rposition(|token| token.word == "IS")?;
    let previous = tokens.get(at.checked_sub(1)?)?;
    if [
        ".", "SELECT", "WHERE", "HAVING", "ON", "AND", "OR", "NOT", "WHEN", "THEN", "ELSE", "AS",
        "FROM", "JOIN", "TABLE", "IS", "(", ",", "=",
    ]
    .contains(&previous.word.as_str())
    {
        return None;
    }
    let tail = &tokens[at + 1..];
    let negated = tail.first().is_some_and(|token| token.word == "NOT");
    let remaining = &tail[usize::from(negated)..];
    let distinct = remaining
        .first()
        .is_some_and(|token| token.word == "DISTINCT");
    if distinct
        && !matches!(
            profile,
            "postgres" | "duckdb" | "sqlite" | "sqlserver" | "clickhouse"
        )
    {
        return Some(Vec::new().into());
    }
    let remaining = &remaining[usize::from(distinct)..];
    if remaining.len() > 1 {
        return None;
    }
    // FROM completes the comparison operator. Its RHS continues through the
    // existing scoped expression service, never a relation-target generator.
    if distinct && remaining.first().is_some_and(|token| token.word == "FROM") {
        return None;
    }
    if profile == "sqlite" && !distinct {
        return None;
    }
    let prefix = remaining.first();
    if prefix.is_some_and(|token| token.span.end < cursor) {
        return None;
    }
    let start = prefix.map_or(cursor, |token| token.span.start);
    let mut end = cursor;
    for (offset, ch) in text[cursor..].char_indices() {
        if !(ch.is_alphanumeric() || matches!(ch, '_' | '$' | '@' | '#')) {
            break;
        }
        end = cursor + offset + ch.len_utf8();
        if end - start > 1024 {
            return None;
        }
    }
    let bounded_types = text.len() <= super::local_relations::LOCAL_RELATION_SCAN_MAX_BYTES;
    let schema =
        if !distinct && bounded_types && matches!(profile, "postgres" | "sqlserver" | "clickhouse")
        {
            source()
        } else {
            None
        };
    let truth_tests = if profile == "postgres" && !distinct && bounded_types {
        let fallback;
        let scope = match schema.as_ref() {
            Some(schema) => schema,
            None => {
                fallback = super::document_semantics::local_schema(text, position, "", profile);
                &fallback
            }
        };
        super::local_relations::infer_comparison_left_type(
            text,
            tokens[at].span.start..if negated {
                tail[0].span.end
            } else {
                tokens[at].span.end
            },
            scope,
            "",
            cursor,
            profile,
            None,
        )
        .and_then(|data_type| {
            crate::dialects::common::type_supports_truth_predicates(&data_type, profile)
        })
    } else {
        None
    };
    let truth_allowed = truth_tests.unwrap_or(true);
    let labels: Vec<&str> = if distinct {
        vec!["FROM"]
    } else {
        let mut labels = vec!["NULL"];
        if !negated {
            labels.push("NOT NULL");
        }
        if truth_allowed && matches!(profile, "postgres" | "mysql" | "duckdb" | "hive") {
            for label in ["TRUE", "FALSE"] {
                labels.push(label);
            }
            if profile != "hive" {
                labels.push("UNKNOWN");
            }
            if !negated {
                labels.extend(["NOT TRUE", "NOT FALSE"]);
                if profile != "hive" {
                    labels.push("NOT UNKNOWN");
                }
            }
        }
        if matches!(profile, "postgres" | "duckdb")
            || profile == "sqlserver"
                && schema
                    .as_ref()
                    .and_then(|schema| schema.server_version_tuple())
                    .is_some_and(|version| version >= (16, 0))
        {
            labels.push("DISTINCT FROM");
            if !negated {
                labels.push("NOT DISTINCT FROM");
            }
        }
        if profile == "clickhouse"
            && schema
                .as_ref()
                .and_then(|schema| schema.server_version_tuple())
                .is_some_and(|version| version >= (25, 10))
        {
            labels.push(if negated {
                "DISTINCT FROM"
            } else {
                "NOT DISTINCT FROM"
            });
        }
        labels
    };
    let insertion = prefix.is_none() && !before.chars().last().is_none_or(char::is_whitespace);
    let range = tower_lsp::lsp_types::Range {
        start: crate::position::lsp_position_at_end(&text[..start]),
        end: crate::position::lsp_position_at_end(&text[..end]),
    };
    let constraints = super::completion_constraints::CompletionKeywordConstraints {
        truth_tests,
        slot: if distinct {
            "distinct-from"
        } else if negated {
            "is-not"
        } else {
            "is"
        },
        allowed_keywords: labels.iter().map(|label| (*label).to_owned()).collect(),
    };
    let items = labels
        .into_iter()
        .filter(|label| prefix.is_none_or(|token| label.starts_with(&token.word)))
        .enumerate()
        .map(|(index, label)| {
            let insert = format!("{}{label}", if insertion { " " } else { "" });
            CompletionItem {
                label: label.into(),
                kind: Some(CompletionItemKind::KEYWORD),
                detail: Some(format!("{profile} IS predicate")),
                sort_text: Some(format!("0:{index:02}")),
                filter_text: Some(if prefix.is_some() {
                    before[start..].into()
                } else if insertion {
                    tokens.last().unwrap().word.clone()
                } else {
                    label.into()
                }),
                insert_text: Some(insert.clone()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: insert,
                })),
                ..Default::default()
            }
        })
        .collect();
    Some(super::completion_context::CompletionCandidates {
        items,
        is_incomplete: false,
        constraints: Some(constraints),
        value_type: None,
        value_role: None,
        logical_continuation: None,
        pattern_escape: None,
    })
}

#[cfg(test)]
fn completions(
    text: &str,
    position: Position,
    profile: &str,
    source: impl FnOnce() -> Option<crate::schema::Schema>,
) -> Option<Vec<CompletionItem>> {
    completion_candidates(text, position, profile, source).map(|result| result.items)
}

fn tail_tokens(mask: &str, source: &str) -> Option<Vec<TailToken>> {
    let mut end = mask.len();
    let mut tokens = Vec::new();
    while end > 0 && tokens.len() < 8 {
        let text = mask[..end].trim_end();
        end = text.len();
        if end == 0 {
            break;
        }
        let (mut start, last) = text.char_indices().next_back()?;
        if last.is_alphanumeric() || matches!(last, '_' | '$' | '@' | '#') {
            for (index, ch) in text[..start].char_indices().rev() {
                if !(ch.is_alphanumeric() || matches!(ch, '_' | '$' | '@' | '#')) {
                    break;
                }
                start = index;
            }
        }
        if end - start > 1024 {
            return None;
        }
        let word = if source[start..end] == mask[start..end] {
            mask[start..end].to_ascii_uppercase()
        } else {
            "<opaque>".into()
        };
        tokens.push(TailToken {
            word,
            span: start..end,
        });
        end = start;
    }
    tokens.reverse();
    Some(tokens)
}

#[cfg(test)]
#[path = "tests/is_predicate_completion.rs"]
mod tests;
