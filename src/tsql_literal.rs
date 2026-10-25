//! Constant spelling shared by T-SQL header metadata and diagnostics. This
//! recognizes syntax only, not target-type conversions or numeric ranges.
pub(crate) fn end_at(text: &str, start: usize) -> Option<usize> {
    let source = text.get(start..)?;
    if source
        .get(..4)
        .is_some_and(|word| word.eq_ignore_ascii_case("NULL"))
        && source[4..]
            .chars()
            .next()
            .is_none_or(|ch| !crate::tsql_identifier::is_name_character(ch))
    {
        return Some(start + 4);
    }
    let quote = if source.starts_with("N'") || source.starts_with("n'") {
        start + 1
    } else {
        start
    };
    if text.as_bytes().get(quote) == Some(&b'\'') {
        let mut at = quote + 1;
        while at < text.len() {
            let ch = text[at..].chars().next()?;
            at += ch.len_utf8();
            if ch == '\'' {
                if text.as_bytes().get(at) == Some(&b'\'') {
                    at += 1;
                } else {
                    return Some(at);
                }
            }
        }
        return None;
    }
    let length = source
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '+' | '-' | '_'))
        .map(|(at, ch)| at + ch.len_utf8())
        .last()?;
    numeric(&source[..length]).then_some(start + length)
}
fn numeric(text: &str) -> bool {
    let bytes = text.as_bytes();
    if text.starts_with("0x") || text.starts_with("0X") {
        return bytes[2..].iter().all(u8::is_ascii_hexdigit);
    }
    let mut at = usize::from(bytes.first().is_some_and(|ch| matches!(ch, b'+' | b'-')));
    let first = at;
    while bytes.get(at).is_some_and(u8::is_ascii_digit) {
        at += 1;
    }
    let mut digits = at - first;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        let first = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        digits += at - first;
    }
    if digits == 0 {
        return false;
    }
    if bytes.get(at).is_some_and(|ch| matches!(ch, b'e' | b'E')) {
        at += 1;
        if bytes.get(at).is_some_and(|ch| matches!(ch, b'+' | b'-')) {
            at += 1;
        }
        let first = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if at == first {
            return false;
        }
    }
    at == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tsql_literals_keep_exact_boundaries_and_reject_foreign_forms() {
        for raw in [
            "NULL",
            "-1",
            "+1",
            ".5",
            "1.25e-3",
            "0x",
            "0Xff",
            "'a''b'",
            "N'金额,AS\\'",
            "'\r\n😀'",
        ] {
            assert_eq!(end_at(&format!("{raw}, next"), 0), Some(raw.len()), "{raw}");
        }
        for raw in [
            "TRUE",
            "FALSE",
            "E'\\n'",
            "DATE'2026-01-01'",
            "$q$value$q$",
            "1abc",
            "1e+",
            "1-2",
            "1_2",
            "'unterminated",
        ] {
            assert!(end_at(raw, 0).is_none(), "{raw}");
        }
    }
}
