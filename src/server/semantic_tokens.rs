//! Semantic tokens for the SQL language server.
use super::document_structure::position_for_offset;
use crate::parser::SqlParser;
use crate::schema::Schema;
use std::collections::HashSet;
use tower_lsp::lsp_types::*;
pub(super) fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::NAMESPACE,
            SemanticTokenType::CLASS,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::VARIABLE,
        ],
        token_modifiers: Vec::new(),
    }
}

pub(super) fn schema_semantic_tokens(text: &str, schema: Option<&Schema>) -> Vec<SemanticToken> {
    let Some(schema) = schema else {
        return Vec::new();
    };
    let tables = schema
        .tables
        .iter()
        .map(|table| table.name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let columns = schema
        .tables
        .iter()
        .flat_map(|table| table.columns.iter())
        .map(|column| column.name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let functions = schema
        .functions
        .iter()
        .map(|function| function.name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let ctes = SqlParser::new()
        .extract_common_table_expressions(text)
        .into_iter()
        .map(|cte| cte.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let database = schema.database.to_ascii_lowercase();

    let mut absolute = Vec::new();
    for (identifier, start, end) in sql_identifier_tokens(text) {
        let normalized = identifier.to_ascii_lowercase();
        let following_open_parenthesis = text[end..]
            .chars()
            .find(|character| !character.is_whitespace())
            == Some('(');
        let token_type = if functions.contains(&normalized) && following_open_parenthesis {
            3
        } else if tables.contains(&normalized) {
            1
        } else if columns.contains(&normalized) {
            2
        } else if ctes.contains(&normalized) {
            4
        } else if normalized == database {
            0
        } else {
            continue;
        };
        let position = position_for_offset(text, start);
        let length = text[start..end].encode_utf16().count() as u32;
        if length > 0 {
            absolute.push((position.line, position.character, length, token_type));
        }
    }
    absolute.sort_unstable();
    absolute.dedup();

    let mut previous_line = 0;
    let mut previous_character = 0;
    absolute
        .into_iter()
        .map(|(line, character, length, token_type)| {
            let delta_line = line - previous_line;
            let delta_start = if delta_line == 0 {
                character - previous_character
            } else {
                character
            };
            previous_line = line;
            previous_character = character;
            SemanticToken {
                delta_line,
                delta_start,
                length,
                token_type,
                token_modifiers_bitset: 0,
            }
        })
        .collect()
}

pub(super) fn sql_identifier_tokens(text: &str) -> Vec<(String, usize, usize)> {
    let mut tokens = Vec::new();
    let mut characters = text.char_indices().peekable();
    let mut quote = None;
    let mut line_comment = false;
    let mut block_comment = false;

    while let Some((index, character)) = characters.next() {
        if line_comment {
            if character == '\n' {
                line_comment = false;
            }
            continue;
        }
        if block_comment {
            if character == '*' && characters.peek().is_some_and(|(_, next)| *next == '/') {
                characters.next();
                block_comment = false;
            }
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                if characters
                    .peek()
                    .is_some_and(|(_, next)| *next == active_quote)
                {
                    characters.next();
                } else {
                    quote = None;
                }
            }
            continue;
        }
        if character == '-' && characters.peek().is_some_and(|(_, next)| *next == '-') {
            characters.next();
            line_comment = true;
            continue;
        }
        if character == '/' && characters.peek().is_some_and(|(_, next)| *next == '*') {
            characters.next();
            block_comment = true;
            continue;
        }
        if matches!(character, '\'' | '"' | '`') {
            quote = Some(character);
            continue;
        }
        if !(character == '_' || character.is_alphabetic()) {
            continue;
        }

        let start = index;
        let mut end = index + character.len_utf8();
        while let Some((next_index, next)) = characters.peek().copied() {
            if !(next == '_' || next == '$' || next.is_alphanumeric()) {
                break;
            }
            characters.next();
            end = next_index + next.len_utf8();
        }
        tokens.push((text[start..end].to_string(), start, end));
    }
    tokens
}
