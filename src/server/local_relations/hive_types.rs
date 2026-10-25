//! Bounded metadata type decomposition. Nested commas and quoted struct names
//! are data, never SQL relation names or argument separators.
use super::read_local_identifier_after;

#[derive(Debug, PartialEq)]
pub(super) enum Kind {
    Array(String),
    Map(String, String),
    Struct(Vec<(String, String)>),
}

pub(super) fn split(source: &str, separator: char) -> Option<Vec<&str>> {
    if source.len() > 16 * 1024 {
        return None;
    }
    let mut stack = Vec::new();
    let mut quote = None;
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut chars = source.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        if let Some(close) = quote {
            if ch == close {
                if chars.peek().is_some_and(|(_, c)| *c == close) {
                    chars.next();
                } else {
                    quote = None;
                }
            } else if ch == '\\' {
                chars.next();
            }
            continue;
        }
        match ch {
            '`' | '"' | '\'' => quote = Some(ch),
            '<' | '(' | '[' => {
                if stack.len() >= 32 {
                    return None;
                }
                stack.push(ch);
            }
            '>' | ')' | ']' => {
                if stack.pop()
                    != Some(match ch {
                        '>' => '<',
                        ')' => '(',
                        _ => '[',
                    })
                {
                    return None;
                }
            }
            _ if ch == separator && stack.is_empty() => {
                pieces.push(source[start..at].trim());
                start = at + ch.len_utf8();
                if pieces.len() >= 256 {
                    return None;
                }
            }
            _ => {}
        }
    }
    if quote.is_some() || !stack.is_empty() {
        return None;
    }
    pieces.push(source[start..].trim());
    (!pieces.iter().any(|piece| piece.is_empty())).then_some(pieces)
}
pub(super) fn kind(ty: &str) -> Option<Kind> {
    let ty = ty.trim();
    if ty.len() > 16 * 1024 {
        return None;
    }
    if let Some(element) = ty
        .strip_suffix("[]")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        split(element, ',')?;
        return Some(Kind::Array(element.into()));
    }
    let open = ty.find('<')?;
    let body = ty[open + 1..].strip_suffix('>')?;
    let parts = split(body, ',')?;
    match ty[..open].trim().to_ascii_lowercase().as_str() {
        "array" if parts.len() == 1 => Some(Kind::Array(parts[0].into())),
        "map" if parts.len() == 2 => Some(Kind::Map(parts[0].into(), parts[1].into())),
        "struct" => {
            let mut fields = Vec::new();
            for field in parts {
                let (name, _, end) = read_local_identifier_after(field, 0)?;
                let value = field[end..].trim_start().strip_prefix(':')?.trim();
                if value.is_empty() {
                    return None;
                }
                fields.push((name, value.into()));
            }
            Some(Kind::Struct(fields))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_hive_collection_types_keep_decimal_commas_and_quoted_field_names() {
        assert_eq!(
            kind(" Array < Map<string, decimal(18,2)> > "),
            Some(Kind::Array("Map<string, decimal(18,2)>".into()))
        );
        assert_eq!(
            kind("struct<`a,b:c`:array<bigint>,`价``格`:decimal(18,2)>"),
            Some(Kind::Struct(vec![
                ("a,b:c".into(), "array<bigint>".into()),
                ("价`格".into(), "decimal(18,2)".into())
            ]))
        );
        assert_eq!(kind("text[]"), Some(Kind::Array("text".into())));
    }
    #[test]
    fn malformed_or_oversized_hive_type_shapes_do_not_publish_partial_fields() {
        for ty in [
            "array<>",
            "map<string>",
            "map<string,int,boolean>",
            "array<map<string,int>",
            "struct<a:int,b>",
            "struct<`unfinished:int>",
        ] {
            assert!(kind(ty).is_none(), "{ty}");
        }
        assert!(kind(&format!("array<{}>", "x".repeat(16 * 1024))).is_none());
        assert!(kind(&format!(
            "array<{}int{}>",
            "array<".repeat(40),
            ">".repeat(40)
        ))
        .is_none());
    }
}
