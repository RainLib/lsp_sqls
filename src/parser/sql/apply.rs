//! The generic grammar has CROSS JOIN but no APPLY production. Adapt only
//! parser input; raw APPLY semantics and source token ranges stay available.
use super::types::SqlParser;

pub(super) fn adapt_apply_joins(mask: &str, output: &mut [u8]) {
    for introducer in ["CROSS", "OUTER"] {
        let mut cursor = 0;
        while let Some(at) = SqlParser::next_keyword_position(mask, introducer, cursor) {
            cursor = at + introducer.len();
            let apply = SqlParser::skip_whitespace(mask, cursor);
            if !mask[apply..].starts_with("APPLY")
                || !SqlParser::is_keyword_at(mask, apply, "APPLY")
            {
                continue;
            }
            output[at..at + 5].copy_from_slice(b"CROSS");
            output[apply..apply + 5].copy_from_slice(b"JOIN ");
        }
    }
}
impl SqlParser {
    pub(super) fn apply_keyword_range(
        node: tree_sitter::Node<'_>,
        source: &str,
    ) -> Option<(usize, usize)> {
        let start = node.start_byte();
        (node.kind() == "keyword_join"
            && source
                .get(start..start + 5)
                .is_some_and(|word| word.eq_ignore_ascii_case("APPLY")))
        .then_some((start, start + 5))
    }
}
