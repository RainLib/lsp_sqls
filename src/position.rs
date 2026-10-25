use tower_lsp::lsp_types::{Diagnostic, Position};

/// Bounded coordinates for one immutable parser source. ASCII columns need no
/// conversion; non-ASCII corrections and line starts are indexed once.
pub(crate) struct SourcePositions {
    lines: Vec<usize>,
    corrections: Vec<(usize, usize)>,
}
impl SourcePositions {
    pub(crate) fn new(source: &str) -> Option<Self> {
        if source.len() > 512 * 1024 {
            return None;
        }
        let mut lines = vec![0];
        let mut corrections = Vec::new();
        let mut loss = 0;
        for (at, ch) in source.char_indices() {
            if ch == '\n' {
                lines.push(at + 1);
            }
            if !ch.is_ascii() {
                loss += ch.len_utf8() - ch.len_utf16();
                corrections.push((at + ch.len_utf8(), loss));
            }
            if lines.len() + corrections.len() > 32768 {
                return None;
            }
        }
        Some(Self { lines, corrections })
    }
    fn loss(&self, at: usize) -> usize {
        let count = self.corrections.partition_point(|(end, _)| *end <= at);
        count.checked_sub(1).map_or(0, |i| self.corrections[i].1)
    }
    pub(crate) fn position(&self, source: &str, point: Position) -> Position {
        let Some(&start) = self.lines.get(point.line as usize) else {
            return Position {
                line: point.line,
                character: 0,
            };
        };
        let end = self
            .lines
            .get(point.line as usize + 1)
            .map_or(source.len(), |next| next - 1);
        let mut at = start + (point.character as usize).min(end - start);
        while !source.is_char_boundary(at) {
            at -= 1;
        }
        Position {
            line: point.line,
            character: (at - start - (self.loss(at) - self.loss(start))) as u32,
        }
    }
    pub(crate) fn offset(&self, source: &str, at: usize) -> Position {
        let at = at.min(source.len());
        let row = self.lines.partition_point(|start| *start <= at) - 1;
        self.position(
            source,
            Position {
                line: row as u32,
                character: (at - self.lines[row]) as u32,
            },
        )
    }
}

/// Convert a byte-based position (as used by tree-sitter) to an LSP UTF-16
/// position. The byte column is interpreted against the original source line.
pub fn byte_position_to_lsp_position(source: &str, position: Position) -> Position {
    let line = source.split('\n').nth(position.line as usize).unwrap_or("");

    Position {
        line: position.line,
        character: byte_column_to_utf16_column(line, position.character as usize),
    }
}

pub fn byte_column_to_utf16_column(line: &str, byte_column: usize) -> u32 {
    let mut byte_column = byte_column.min(line.len());
    while !line.is_char_boundary(byte_column) {
        byte_column -= 1;
    }

    line[..byte_column].encode_utf16().count() as u32
}

/// Convert an LSP UTF-16 position to the byte-based column expected by tree-sitter.
pub fn lsp_position_to_byte_position(source: &str, position: Position) -> Position {
    let mut last_line = "";
    let mut last_line_index = 0u32;

    for (line_index, line) in source.split('\n').enumerate() {
        last_line = line;
        last_line_index = line_index as u32;
        if line_index == position.line as usize {
            return Position {
                line: position.line,
                character: utf16_column_to_byte_offset(line, position.character) as u32,
            };
        }
    }

    Position {
        line: last_line_index,
        character: last_line.len() as u32,
    }
}

pub fn utf16_column_to_byte_offset(line: &str, character: u32) -> usize {
    let target_units = character as usize;
    let mut current_units = 0usize;

    for (byte_index, ch) in line.char_indices() {
        if current_units >= target_units {
            return byte_index;
        }

        let next_units = current_units + ch.len_utf16();
        if next_units > target_units {
            return byte_index;
        }

        current_units = next_units;
    }

    line.len()
}

pub fn byte_position_at_end(source: &str) -> Position {
    let mut line = 0;
    let mut character = 0;

    for ch in source.chars() {
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf8() as u32;
        }
    }

    Position { line, character }
}

pub fn lsp_position_at_end(source: &str) -> Position {
    byte_position_to_lsp_position(source, byte_position_at_end(source))
}

pub fn diagnostic_reaches_position(diagnostic: &Diagnostic, position: Position) -> bool {
    diagnostic.range.end.line > position.line
        || (diagnostic.range.end.line == position.line
            && diagnostic.range.end.character >= position.character.saturating_sub(1))
        || (diagnostic.range.start.line == position.line
            && diagnostic.range.start.character >= position.character.saturating_sub(1))
}

pub fn cursor_token_prefix(
    source: &str,
    position: Position,
    is_token_char: impl Fn(char) -> bool,
) -> String {
    let position = lsp_position_to_byte_position(source, position);
    let line = source.split('\n').nth(position.line as usize).unwrap_or("");
    let byte_index = position.character.min(line.len() as u32) as usize;
    let mut start = byte_index.min(line.len());

    while start > 0 {
        let Some(character) = line[..start].chars().next_back() else {
            break;
        };
        if !is_token_char(character) {
            break;
        }
        start -= character.len_utf8();
    }

    line[start..byte_index.min(line.len())]
        .trim_matches('"')
        .trim_matches('\'')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_byte_columns_to_utf16_on_the_requested_line() {
        let source = "SELECT 1;\nSELECT '😀中文', value";
        let second_line = source.lines().nth(1).expect("second line");
        let value_byte_column = second_line.find("value").expect("value column");

        assert_eq!(
            byte_position_to_lsp_position(
                source,
                Position {
                    line: 1,
                    character: value_byte_column as u32,
                },
            ),
            Position {
                line: 1,
                character: second_line[..value_byte_column].encode_utf16().count() as u32,
            }
        );
        assert_ne!(
            value_byte_column as u32,
            second_line[..value_byte_column].encode_utf16().count() as u32,
            "the fixture must distinguish UTF-8 byte columns from UTF-16 columns"
        );
    }

    #[test]
    fn converts_eof_to_utf16_after_multiline_non_ascii_text() {
        let source = "SELECT '中文';\nSELECT '😀'";

        assert_eq!(
            lsp_position_at_end(source),
            Position {
                line: 1,
                character: "SELECT '😀'".encode_utf16().count() as u32,
            }
        );
    }

    #[test]
    fn cursor_prefix_keeps_unicode_identifier_boundaries() {
        let source = "SELECT 订单金";
        assert_eq!(
            cursor_token_prefix(source, lsp_position_at_end(source), |character| {
                character.is_alphanumeric() || character == '_'
            }),
            "订单金"
        );
    }
}
#[test]
fn variable_scopes_index_matches_utf16_reference_at_every_byte_and_line() {
    let source = "ASCII\r\n金额😀e\u{301}\r\n\n最后\n";
    let index = SourcePositions::new(source).unwrap();
    for (row, line) in source.split('\n').enumerate() {
        for column in 0..line.len() + 3 {
            let point = Position {
                line: row as u32,
                character: column as u32,
            };
            assert_eq!(
                index.position(source, point),
                byte_position_to_lsp_position(source, point)
            );
        }
    }
    for at in 0..source.len() + 3 {
        let mut clamped = at.min(source.len());
        while !source.is_char_boundary(clamped) {
            clamped -= 1;
        }
        assert_eq!(
            index.offset(source, at),
            lsp_position_at_end(&source[..clamped])
        );
    }
}
#[test]
fn variable_scopes_index_caps_memory_and_parser_edits_replace_old_coordinates() {
    assert!(SourcePositions::new(&"a".repeat(512 * 1024 + 1)).is_none());
    assert!(SourcePositions::new(&"\n".repeat(32768)).is_none());
    assert!(SourcePositions::new(&"值".repeat(32768)).is_none());
    let mut parser = crate::SqlParser::new();
    for source in [
        "/*😀*/ SELECT 1;",
        "SELECT 1;\r\n/*金额🧭*/SELECT 2;",
        "SELECT 3;",
    ] {
        let tree = parser.parse_syntax_tree(source).unwrap();
        let range = parser.node_range(tree.root_node());
        assert_eq!(range.end, lsp_position_at_end(source));
    }
}
