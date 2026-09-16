use super::types::SqlParser;
use crate::placeholder::normalize_sql_placeholders;

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

    pub(super) fn starts_hash_comment(source: &str, index: usize) -> bool {
        if !source[index..].starts_with('#') {
            return false;
        }

        if source[index..].starts_with("#>") || source[index..].starts_with("#-") {
            return false;
        }

        index == 0
            || source[..index]
                .chars()
                .next_back()
                .is_none_or(|ch| ch.is_whitespace())
    }

    pub(super) fn mask_quoted_region(
        source: &str,
        output: &mut String,
        index: usize,
        quote: char,
    ) -> usize {
        let mut index = Self::push_masked_range(source, output, index, index + quote.len_utf8());

        while index < source.len() {
            let Some(ch) = source[index..].chars().next() else {
                break;
            };
            index = Self::push_masked_range(source, output, index, index + ch.len_utf8());

            if ch == '\\' && quote == '\'' && index < source.len() {
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
        let normalized_source = normalize_sql_placeholders(source);
        let source = normalized_source.as_str();
        let mut output = String::with_capacity(source.len());
        let mut index = 0;

        while index < source.len() {
            if source[index..].starts_with("--") {
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
                continue;
            }

            if Self::starts_hash_comment(source, index) {
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
                continue;
            }

            if source[index..].starts_with("/*") {
                index = Self::push_masked_range(source, &mut output, index, index + 2);
                while index < source.len() {
                    if source[index..].starts_with("*/") {
                        index = Self::push_masked_range(source, &mut output, index, index + 2);
                        break;
                    }
                    let Some(ch) = source[index..].chars().next() else {
                        break;
                    };
                    index =
                        Self::push_masked_range(source, &mut output, index, index + ch.len_utf8());
                }
                continue;
            }

            let Some(ch) = source[index..].chars().next() else {
                break;
            };

            if ch == '$' {
                if let Some(next_index) =
                    Self::mask_dollar_quoted_region(source, &mut output, index)
                {
                    index = next_index;
                    continue;
                }
            }

            if matches!(ch, '\'' | '"' | '`') {
                index = Self::mask_quoted_region(source, &mut output, index, ch);
                continue;
            }

            if ch == '[' {
                index = Self::mask_bracketed_identifier(source, &mut output, index);
                continue;
            }

            output.push(ch);
            index += ch.len_utf8();
        }

        output
    }
}
