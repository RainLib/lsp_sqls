use super::types::SqlParser;
use crate::placeholder::{normalize_sql_placeholders_for_dialect, SqlPlaceholderDialect};
use crate::sql_lexical::{lexical_rules_for, LexicalRules, LEGACY_LEXICAL_RULES};

impl SqlParser {
    pub(crate) fn is_identifier_char(ch: char) -> bool {
        ch.is_alphanumeric() || matches!(ch, '_' | '$' | '@' | '#')
    }

    pub(super) fn push_masked_char(output: &mut String, ch: char) {
        if matches!(ch, '\n' | '\r') {
            output.push(ch);
        } else {
            for _ in 0..ch.len_utf8() {
                output.push(' ');
            }
        }
    }

    pub(super) fn push_masked_range(
        source: &str,
        output: &mut String,
        mut index: usize,
        end: usize,
    ) -> usize {
        while index < end {
            let Some(ch) = source[index..].chars().next() else {
                break;
            };
            Self::push_masked_char(output, ch);
            index += ch.len_utf8();
        }
        index
    }

    pub(super) fn mask_quoted_region(
        source: &str,
        output: &mut String,
        index: usize,
        quote: char,
        escaped: bool,
    ) -> usize {
        let mut index = Self::push_masked_range(source, output, index, index + quote.len_utf8());

        while index < source.len() {
            let Some(ch) = source[index..].chars().next() else {
                break;
            };
            index = Self::push_masked_range(source, output, index, index + ch.len_utf8());

            if escaped && ch == '\\' && index < source.len() {
                let Some(escaped) = source[index..].chars().next() else {
                    break;
                };
                index = Self::push_masked_range(source, output, index, index + escaped.len_utf8());
                continue;
            }

            if ch == quote {
                if source[index..].starts_with(quote) {
                    index =
                        Self::push_masked_range(source, output, index, index + quote.len_utf8());
                    continue;
                }
                break;
            }
        }

        index
    }

    pub(super) fn mask_bracketed_identifier(
        source: &str,
        output: &mut String,
        index: usize,
    ) -> usize {
        let mut index = Self::push_masked_range(source, output, index, index + 1);

        while index < source.len() {
            let Some(ch) = source[index..].chars().next() else {
                break;
            };
            index = Self::push_masked_range(source, output, index, index + ch.len_utf8());

            if ch == ']' {
                if source[index..].starts_with(']') {
                    index = Self::push_masked_range(source, output, index, index + 1);
                    continue;
                }
                break;
            }
        }

        index
    }

    pub(super) fn dollar_quote_tag_at(source: &str, index: usize) -> Option<&str> {
        let rest = source.get(index..)?;
        let bytes = rest.as_bytes();
        if bytes.first() != Some(&b'$') {
            return None;
        }

        let mut end = 1;
        while end < bytes.len() {
            let byte = bytes[end];
            if byte == b'$' {
                return Some(&rest[..=end]);
            }
            if end == 1 && !(byte == b'_' || byte.is_ascii_alphabetic()) {
                return None;
            }
            if end > 1 && !(byte == b'_' || byte.is_ascii_alphanumeric()) {
                return None;
            }
            end += 1;
        }

        None
    }

    pub(super) fn mask_dollar_quoted_region(
        source: &str,
        output: &mut String,
        index: usize,
    ) -> Option<usize> {
        let tag = Self::dollar_quote_tag_at(source, index)?;
        let body_start = index + tag.len();
        let end = source[body_start..]
            .find(tag)
            .map(|body_end| body_start + body_end + tag.len())
            .unwrap_or(source.len());

        Some(Self::push_masked_range(source, output, index, end))
    }

    pub(crate) fn mask_sql_noise(source: &str) -> String {
        Self::mask_sql_regions(source, false, LEGACY_LEXICAL_RULES, None)
    }

    pub(crate) fn mask_sql_noise_for_dialect(source: &str, dialect: &str) -> String {
        Self::mask_sql_regions(source, false, lexical_rules_for(dialect), None)
    }

    pub(crate) fn mask_sql_operand_context_for_dialect(source: &str, dialect: &str) -> String {
        Self::mask_sql_regions(source, true, lexical_rules_for(dialect), None)
    }

    pub(crate) fn mask_sql_value_context_and_comments(
        source: &str,
        dialect: &str,
        visitor: &mut dyn FnMut(std::ops::Range<usize>),
    ) -> String {
        Self::mask_sql_regions(source, true, lexical_rules_for(dialect), Some(visitor))
    }

    /// Visit only actual comment ranges while producing the same byte mask.
    /// Consumers can collect directives without rescanning opaque SQL strings.
    pub(crate) fn mask_sql_noise_and_comments_for_dialect(
        source: &str,
        dialect: &str,
        visitor: &mut dyn FnMut(std::ops::Range<usize>),
    ) -> String {
        Self::mask_sql_regions(source, false, lexical_rules_for(dialect), Some(visitor))
    }

    pub(super) fn mask_sql_operand_context(source: &str, dialect: SqlPlaceholderDialect) -> String {
        let rules = LexicalRules {
            nested_comments: matches!(
                dialect,
                SqlPlaceholderDialect::Postgres | SqlPlaceholderDialect::SqlServer
            ),
            ..LEGACY_LEXICAL_RULES
        };
        Self::mask_sql_regions(source, true, rules, None)
    }

    fn mask_sql_regions(
        source: &str,
        operand_markers: bool,
        rules: LexicalRules,
        mut comments: Option<&mut dyn FnMut(std::ops::Range<usize>)>,
    ) -> String {
        let normalized_source = normalize_sql_placeholders_for_dialect(source, rules.placeholder);
        let source = normalized_source.as_str();
        let mut output = String::with_capacity(source.len());
        let mut index = 0;

        while index < source.len() {
            if source[index..].starts_with("--")
                || rules.slash_comments && source[index..].starts_with("//")
            {
                let start = index;
                while index < source.len() {
                    let Some(ch) = source[index..].chars().next() else {
                        break;
                    };
                    index =
                        Self::push_masked_range(source, &mut output, index, index + ch.len_utf8());
                    if ch == '\n' {
                        break;
                    }
                }
                if let Some(visitor) = comments.as_mut() {
                    visitor(start..index);
                }
                continue;
            }

            if rules.hash_comment_at(source, index) {
                let start = index;
                while index < source.len() {
                    let Some(ch) = source[index..].chars().next() else {
                        break;
                    };
                    index =
                        Self::push_masked_range(source, &mut output, index, index + ch.len_utf8());
                    if ch == '\n' {
                        break;
                    }
                }
                if let Some(visitor) = comments.as_mut() {
                    visitor(start..index);
                }
                continue;
            }

            if source[index..].starts_with("/*") {
                let start = index;
                index = Self::push_masked_range(source, &mut output, index, index + 2);
                let mut depth = 1;
                while index < source.len() && depth > 0 {
                    if rules.nested_comments && source[index..].starts_with("/*") {
                        index = Self::push_masked_range(source, &mut output, index, index + 2);
                        depth += 1;
                    } else if source[index..].starts_with("*/") {
                        index = Self::push_masked_range(source, &mut output, index, index + 2);
                        depth -= 1;
                    } else {
                        let Some(ch) = source[index..].chars().next() else {
                            break;
                        };
                        index = Self::push_masked_range(
                            source,
                            &mut output,
                            index,
                            index + ch.len_utf8(),
                        );
                    }
                }
                if let Some(visitor) = comments.as_mut() {
                    visitor(start..index);
                }
                continue;
            }

            let Some(ch) = source[index..].chars().next() else {
                break;
            };

            if rules.dollar_quotes && ch == '$' {
                if let Some(next_index) =
                    Self::mask_dollar_quoted_region(source, &mut output, index)
                {
                    if operand_markers {
                        output.replace_range(index..index + 1, "x");
                    }
                    index = next_index;
                    continue;
                }
            }

            if matches!(ch, '\'' | '"' | '`') {
                let start = index;
                let escaped = rules.escapes_at(source, index, ch);
                index = Self::mask_quoted_region(source, &mut output, index, ch, escaped);
                if operand_markers {
                    output.replace_range(start..start + 1, "x");
                }
                continue;
            }

            if rules.bracket_identifiers && ch == '[' {
                let start = index;
                index = Self::mask_bracketed_identifier(source, &mut output, index);
                if operand_markers {
                    output.replace_range(start..start + 1, "x");
                }
                continue;
            }

            output.push(ch);
            index += ch.len_utf8();
        }

        output
    }
}
