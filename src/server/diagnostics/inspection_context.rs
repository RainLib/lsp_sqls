//! One dialect mask, directive stream and bounded coordinate index per request.
use super::statement_structure::{MissingJoin, Mutation, Structure};
use crate::{parser::SqlParser, position::SourcePositions};
use std::cell::OnceCell;
use std::ops::Range as Bytes;
use tower_lsp::lsp_types::Range;

const CODES: [&str; 5] = ["OXIDE001", "OXIDE002", "OXIDE003", "OXIDE004", "OXIDE005"];
pub(in crate::server) struct Context<'a> {
    pub(super) text: &'a str,
    pub(super) dialect: &'a str,
    pub(super) normalized: String,
    positions: Option<SourcePositions>,
    directives: Vec<(Bytes<usize>, u8)>,
    structure: OnceCell<Structure>,
    syntax: Option<&'a tree_sitter::Tree>,
    fallback_syntax: OnceCell<Option<tree_sitter::Tree>>,
}
fn directives(comment: &str) -> u8 {
    let mut flags = 0;
    let mut words = comment
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter(|word| !word.is_empty())
        .peekable();
    while let Some(word) = words.next() {
        if word.eq_ignore_ascii_case("NOINSPECTION") {
            while let Some(i) = words.peek().and_then(|word| {
                CODES
                    .iter()
                    .position(|code| word.eq_ignore_ascii_case(code))
            }) {
                flags |= 1 << i;
                words.next();
            }
        }
    }
    flags
}
impl<'a> Context<'a> {
    pub(in crate::server) fn new(text: &'a str, dialect: &'a str) -> Self {
        Self::with_syntax(text, dialect, None)
    }
    pub(in crate::server) fn with_syntax(
        text: &'a str,
        dialect: &'a str,
        syntax: Option<&'a tree_sitter::Tree>,
    ) -> Self {
        let mut spans = Vec::new();
        let normalized =
            SqlParser::mask_sql_noise_and_comments_for_dialect(text, dialect, &mut |range| {
                let flags = directives(&text[range.clone()]);
                if flags != 0 {
                    spans.push((range, flags));
                }
            })
            .to_ascii_uppercase();
        Self {
            text,
            dialect,
            normalized,
            positions: SourcePositions::new(text),
            directives: spans,
            structure: OnceCell::new(),
            syntax,
            fallback_syntax: OnceCell::new(),
        }
    }

    pub(super) fn syntax_tree(&self) -> Option<&tree_sitter::Tree> {
        self.syntax.or_else(|| {
            self.fallback_syntax
                .get_or_init(|| {
                    #[cfg(test)]
                    super::statement_structure::FALLBACK_PARSE_COUNT.with(|n| n.set(n.get() + 1));
                    let mut parser = SqlParser::new_with_placeholder_dialect(
                        crate::sql_lexical::lexical_rules_for(self.dialect).placeholder,
                    );
                    parser.parse_syntax_tree(self.text)
                })
                .as_ref()
        })
    }
    fn structure(&self) -> &Structure {
        self.structure.get_or_init(|| {
            if self.directives.is_empty()
                && !["UPDATE", "DELETE", "JOIN"]
                    .iter()
                    .any(|w| super::contains_sql_keyword(&self.normalized, w))
            {
                return Structure::default();
            }
            let Some(tree) = self.syntax_tree() else {
                return Structure::default();
            };
            Structure::new(
                self.text,
                &self.normalized,
                !self.directives.is_empty(),
                tree,
            )
        })
    }
    pub(in crate::server) fn mutations(
        &self,
        span: &Bytes<usize>,
    ) -> impl Iterator<Item = &Mutation> {
        let all = &self.structure().mutations;
        let first = all.partition_point(|m| m.keyword < span.start);
        let end = span.end;
        all[first..].iter().take_while(move |m| m.keyword < end)
    }
    pub(in crate::server) fn joins(
        &self,
        span: &Bytes<usize>,
    ) -> impl Iterator<Item = &MissingJoin> {
        let all = &self.structure().joins;
        let first = all.partition_point(|m| m.keyword < span.start);
        let end = span.end;
        all[first..].iter().take_while(move |m| m.keyword < end)
    }
    pub(in crate::server) fn range(&self, start: usize, end: usize) -> Range {
        self.positions.as_ref().map_or_else(
            || super::super::document_structure::range_for_offsets(self.text, start, end),
            |index| Range::new(index.offset(self.text, start), index.offset(self.text, end)),
        )
    }
    pub(in crate::server) fn statements(&self) -> impl Iterator<Item = Bytes<usize>> + '_ {
        let mut start = 0;
        self.normalized
            .match_indices(';')
            .map(|(at, _)| at + 1)
            .chain(std::iter::once(self.text.len()))
            .filter_map(move |end| {
                let segment = &self.text[start..end];
                let begin = start + segment.len() - segment.trim_start().len();
                let finish = start + segment.trim_end().len();
                start = end;
                (begin < finish).then_some(begin..finish)
            })
    }
    pub(in crate::server) fn statement_at(&self, at: usize) -> Option<Bytes<usize>> {
        self.statements()
            .find(|span| span.start <= at && at < span.end)
    }
    pub(in crate::server) fn suppressions_at(&self, span: &Bytes<usize>, at: usize) -> u8 {
        let first = self
            .directives
            .partition_point(|(r, _)| r.end <= span.start);
        self.directives[first..]
            .iter()
            .take_while(|(r, _)| r.start < span.end)
            .filter(|(r, _)| self.structure().comment_matches(at, r))
            .fold(0, |flags, (_, next)| flags | next)
    }
    pub(in crate::server) fn suppressions(&self, span: &Bytes<usize>) -> u8 {
        let first = self
            .directives
            .partition_point(|(comment, _)| comment.end <= span.start);
        self.directives[first..]
            .iter()
            .take_while(|(comment, _)| comment.start < span.end)
            .fold(0, |flags, (_, next)| flags | next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspection_regions_context_visits_real_comments_and_shares_the_original_mask() {
        let sql="SELECT '; -- noinspection OXIDE001'; /* noinspection OXIDE002, OXIDE003 */ SELECT * FROM a JOIN b;";
        let context = Context::new(sql, "postgres");
        assert_eq!(
            context.normalized,
            SqlParser::mask_sql_noise_for_dialect(sql, "postgres").to_ascii_uppercase()
        );
        let ranges = context.statements().collect::<Vec<_>>();
        assert_eq!(ranges.len(), 2);
        assert_eq!(context.suppressions(&ranges[0]), 0);
        assert_eq!(context.suppressions(&ranges[1]), 6);
    }
    #[test]
    fn inspection_regions_context_coordinates_keep_astral_crlf_and_rebuilds() {
        for source in ["/*😀*/\r\nSELECT 'a;b'; SELECT * FROM users;", "SELECT 1;"] {
            let context = Context::new(source, "sqlserver");
            for range in context.statements() {
                assert_eq!(
                    context.range(range.start, range.end),
                    crate::server::document_structure::range_for_offsets(
                        source,
                        range.start,
                        range.end
                    )
                );
            }
        }
    }
}
