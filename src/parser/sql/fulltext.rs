//! Adapt only the LANGUAGE prefix at its designated full-text argument slot.
//! The value, original SQL and all byte/newline coordinates remain unchanged.
pub(super) fn adapt_languages(source: &str, mask: &str, output: &mut [u8]) {
    struct Frame {
        slot: Option<usize>,
        argument: usize,
        start: usize,
    }
    let mut frames: Vec<Frame> = Vec::new();
    let bytes = mask.as_bytes();
    for (index, byte) in bytes.iter().copied().enumerate() {
        match byte {
            b'(' => frames.push(Frame {
                slot: language_slot(source, mask, index),
                argument: 0,
                start: index + 1,
            }),
            b')' => {
                frames.pop();
            }
            b',' => {
                if let Some(frame) = frames.last_mut() {
                    frame.argument += 1;
                    frame.start = index + 1;
                }
            }
            b'L' => {
                let Some(frame) = frames.last() else {
                    continue;
                };
                if frame.slot != Some(frame.argument)
                    || !mask[frame.start..index].trim().is_empty()
                    || !bytes[index..].starts_with(b"LANGUAGE")
                    || !bytes.get(index + 8).is_some_and(u8::is_ascii_whitespace)
                {
                    continue;
                }
                let next = mask[index + 8..].trim_start().as_bytes().first().copied();
                if next.is_some_and(|byte| !matches!(byte, b',' | b')')) {
                    output[index..index + 8].fill(b' ');
                }
            }
            _ => {}
        }
    }
}

fn language_slot(source: &str, mask: &str, open: usize) -> Option<usize> {
    let before = mask[..open].trim_end();
    let name = before
        .rsplit(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .next()?;
    let start = before.len() - name.len();
    if before[..start].trim_end().ends_with('.')
        || source
            .get(start..before.len())?
            .bytes()
            .any(|b| !b.is_ascii_alphabetic() && b != b'_')
    {
        return None;
    }
    match name {
        "CONTAINSTABLE" | "FREETEXTTABLE" => Some(3),
        "CONTAINS" | "FREETEXT" => Some(2),
        _ => None,
    }
}
