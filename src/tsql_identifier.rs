//! One bounded T-SQL local-variable spelling for parser adaptation and menus.
//! Unicode 3.2 BMP L*/Nd categories are checked in, without runtime generation.
use crate::tsql_identifier_table::BMP_LETTERS_AND_DECIMALS;

pub(crate) const MAX_NAME_UTF16_UNITS: usize = 128;
pub(crate) const MAX_NAME_UTF8_BYTES: usize = MAX_NAME_UTF16_UNITS * 3;

pub(crate) fn is_name_character(ch: char) -> bool {
    if ch.is_ascii() {
        return ch.is_ascii_alphanumeric() || matches!(ch, '_' | '@' | '$' | '#');
    }
    let point = ch as u32;
    if point > 0xFFFF {
        return false;
    }
    BMP_LETTERS_AND_DECIMALS
        .binary_search_by(|(start, end)| {
            if point < u32::from(*start) {
                std::cmp::Ordering::Greater
            } else if point > u32::from(*end) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

// A lone @ is an editor prefix, but never a complete metadata/normalization name.
pub(crate) fn prefix_end(text: &str, start: usize) -> Option<usize> {
    let source = text.get(start..)?;
    if !source.starts_with('@')
        || source.starts_with("@@")
        || text[..start]
            .chars()
            .next_back()
            .is_some_and(is_name_character)
    {
        return None;
    }
    let (mut units, mut end) = (1, start + 1);
    for (at, ch) in source[1..].char_indices() {
        if !is_name_character(ch) {
            break;
        }
        units += ch.len_utf16();
        if units > MAX_NAME_UTF16_UNITS {
            return None;
        }
        end = start + 1 + at + ch.len_utf8();
    }
    Some(end)
}
pub(crate) fn variable_end(text: &str, start: usize) -> Option<usize> {
    prefix_end(text, start).filter(|end| *end > start + 1)
}
pub(crate) fn is_complete_name(text: &str) -> bool {
    variable_end(text, 0) == Some(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_lookup_matches_dense_unicode_bmp_snapshot() {
        let mut dense = [false; 65536];
        for &(start, end) in BMP_LETTERS_AND_DECIMALS {
            for point in start..=end {
                dense[usize::from(point)] = true;
            }
        }
        for ch in ['_', '@', '$', '#'] {
            dense[ch as usize] = true;
        }
        for (point, expected) in dense.into_iter().enumerate() {
            if let Some(ch) = char::from_u32(point as u32) {
                assert_eq!(is_name_character(ch), expected, "U+{point:04X}");
            } else {
                assert!(!expected);
            }
        }
        for point in [0x10000, 0x10400, 0x1F600, 0x10FFFF] {
            assert!(!is_name_character(char::from_u32(point).unwrap()));
        }
    }
}
