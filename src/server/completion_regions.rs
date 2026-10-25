//! Allocation-free prefix scan. Quoted identifiers remain completion sites.
use super::completion_context::position_to_byte_offset;
use tower_lsp::lsp_types::Position;

pub(super) fn in_literal_or_comment(text: &str, position: Position, dialect: &str) -> bool {
    let cursor = position_to_byte_offset(text, position);
    let bytes = text.as_bytes();
    let rules = crate::sql_lexical::lexical_rules_for(dialect);
    let mut index = 0;
    while index < cursor {
        if bytes[index..].starts_with(b"--")
            || rules.slash_comments && bytes[index..].starts_with(b"//")
            || rules.hash_comment_at(text, index)
        {
            index += 1;
            while index < cursor && !matches!(bytes[index], b'\n' | b'\r') {
                index += 1;
            }
            if index == cursor {
                return true;
            }
        } else if bytes[index..].starts_with(b"/*") {
            index += 2;
            let mut depth = 1;
            while index < cursor && depth > 0 {
                if rules.nested_comments && bytes[index..].starts_with(b"/*") {
                    depth += 1;
                    index += 2;
                } else if bytes[index..].starts_with(b"*/") {
                    depth -= 1;
                    index += 2;
                } else {
                    index += 1;
                }
            }
            if depth > 0 || index > cursor {
                return true;
            }
        } else if bytes[index] == b'[' && rules.bracket_identifiers {
            index += 1;
            while index < cursor {
                if bytes[index] == b']' {
                    if bytes.get(index + 1) == Some(&b']') {
                        index += 2;
                    } else {
                        index += 1;
                        break;
                    }
                } else {
                    index += 1;
                }
            }
        } else if matches!(bytes[index], b'\'' | b'"' | b'`') {
            let quote = bytes[index];
            let literal = quote == b'\'' || quote == b'"' && rules.double_quoted_strings;
            let escaped = rules.escapes_at(text, index, quote as char);
            index += 1;
            let mut closed = false;
            while index < cursor {
                if escaped && bytes[index] == b'\\' {
                    index += 2;
                } else if bytes[index] == quote {
                    if bytes.get(index + 1) == Some(&quote) {
                        index += 2;
                    } else {
                        index += 1;
                        closed = true;
                        break;
                    }
                } else {
                    index += 1;
                }
            }
            if !closed || index > cursor {
                return literal;
            }
        } else if rules.dollar_quotes
            && bytes[index] == b'$'
            && (index == 0
                || !matches!(bytes[index - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$'))
        {
            let mut tag_end = index + 1;
            while bytes
                .get(tag_end)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                tag_end += 1;
            }
            if bytes.get(tag_end) == Some(&b'$')
                && (tag_end == index + 1 || !bytes[index + 1].is_ascii_digit())
            {
                let delimiter = &text[index..=tag_end];
                let body = tag_end + 1;
                let Some(end) = text[body..]
                    .find(delimiter)
                    .map(|offset| body + offset + delimiter.len())
                else {
                    return true;
                };
                if end > cursor {
                    return true;
                }
                index = end;
            } else {
                index += 1;
            }
        } else {
            index += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inside(dialect: &str, marked: &str) -> bool {
        let index = marked.find("<cursor>").unwrap();
        let position = crate::position::lsp_position_at_end(&marked[..index]);
        in_literal_or_comment(&marked.replace("<cursor>", ""), position, dialect)
    }
    #[test]
    fn cursor_regions_respect_escapes_dollar_bodies_and_dialect_comments() {
        for sql in [
            "SELECT 'oxide<cursor>'",
            "SELECT 'it''s <cursor>'",
            "SELECT E'it\\'s <cursor>'",
            "SELECT $q$ u.na<cursor>me $q$",
            "SELECT 1 -- <cursor>",
            "/* a /* b */ <cursor> */ SELECT 1",
        ] {
            assert!(inside("postgres", sql), "{sql}");
        }
        for sql in [
            "SELECT 'oxide'<cursor>",
            "SELECT \"na<cursor>me\"",
            "SELECT 'x' || u.na<cursor>me",
            "SELECT 1 -- comment\n<cursor>",
            "SELECT $q$x$q$<cursor>",
        ] {
            assert!(!inside("postgres", sql), "{sql}");
        }
        assert!(inside("mysql", "SELECT 1 # <cursor>"));
        assert!(inside("mysql", "SELECT 'it\\'s <cursor>'"));
        assert!(!inside("mysql", "SELECT u.`名<cursor>称`"));
        assert!(!inside("postgres", "SELECT payload #>> <cursor>'{a}'"));
        assert!(!inside("tsql", "SELECT [a'b<cursor>c]"));
    }
}
