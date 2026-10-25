//! T-SQL identifier brackets are adapted only in its parser profile. The
//! original source and all byte/UTF-16 coordinates remain authoritative.
use super::types::SqlParser;

pub(super) fn adapt_bracket_identifiers(source: &str, searchable: &str, output: &mut [u8]) {
    for (index, marker) in searchable.bytes().enumerate() {
        if !matches!(marker, b'x' | b'X') || source.as_bytes()[index] != b'[' {
            continue;
        }
        let Some((_, end)) = SqlParser::read_identifier_part(source, index) else {
            continue;
        };
        // Empty brackets are never a delimited identifier. Keep array-like
        // input visible so the dialect validator can reject declarations.
        if end == index + 2 {
            continue;
        }
        // A plain surrogate is unambiguously an identifier. Double quotes
        // compete with string literals in the shared grammar's SELECT terms.
        output[index] = b'p';
        output[end - 1] = b'_';
        for byte in &mut output[index + 1..end - 1] {
            if !matches!(*byte, b'\r' | b'\n') {
                *byte = b'_';
            }
        }
    }
}
