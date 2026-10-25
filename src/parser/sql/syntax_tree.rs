//! Syntax ownership is independent of catalogs, cursor positions and diagnostic
//! rendering. One parser retains at most one bounded tree for its fixed grammar
//! and placeholder dialect. Candidate lists and diagnostics are never cached.
use super::normalization::normalize_for_parser;
use super::{core::minimal_input_edit, types::SqlParser};
use tree_sitter::Tree;

pub(super) const SYNTAX_CACHE_MAX_BYTES: usize = 256 * 1024;
pub(super) const SYNTAX_CACHE_MAX_NODES: usize = 32 * 1024;

impl SqlParser {
    /// An unfinished `g. FROM (...)` is otherwise recovered as a call to
    /// `g.FROM(...)`, swallowing the actual joined input. Hide only that
    /// dangling dot in a request-local tree, preserving all byte coordinates.
    /// Validation and the canonical syntax cache always retain original SQL.
    pub(crate) fn parse_completion_tree(
        &mut self,
        sql: &str,
        position: tower_lsp::lsp_types::Position,
    ) -> Option<Tree> {
        let at = Self::byte_offset_for_position(sql, position).min(sql.len());
        if at > 0
            && sql.as_bytes()[at - 1] == b'.'
            && sql.as_bytes().get(at).is_some_and(u8::is_ascii_whitespace)
        {
            let suffix = &sql[at..];
            if suffix.len() <= 64 * 1024 {
                let mask = Self::mask_sql_operand_context(suffix, self.placeholder_dialect)
                    .to_ascii_uppercase();
                let next = mask.trim_start();
                if next.starts_with("FROM")
                    && next
                        .as_bytes()
                        .get(4)
                        .is_some_and(|b| b.is_ascii_whitespace() || *b == b'(')
                    && next
                        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .any(|word| matches!(word, "JOIN" | "LATERAL"))
                    && Self::mask_sql_operand_context(sql, self.placeholder_dialect).as_bytes()
                        [at - 1]
                        == b'.'
                {
                    let mut normalized =
                        normalize_for_parser(sql, self.placeholder_dialect).into_bytes();
                    normalized[at - 1] = b' ';
                    #[cfg(test)]
                    {
                        self.syntax_parse_count += 1;
                    }
                    return self.parser.parse(
                        String::from_utf8(normalized).expect("dot recovery preserves UTF-8"),
                        None,
                    );
                }
            }
        }
        self.parse_syntax_tree(sql)
    }
    /// Parse tolerant syntax without producing diagnostics. Completion needs
    /// the tree, not another document-wide diagnostic pass on every keystroke.
    pub fn parse_syntax_tree(&mut self, sql: &str) -> Option<Tree> {
        if self.source == sql {
            if let Some(tree) = &self.syntax_tree {
                return Some(tree.clone());
            }
        }
        #[cfg(test)]
        {
            self.syntax_parse_count += 1;
        }
        let normalized = normalize_for_parser(sql, self.placeholder_dialect);
        let tree = if sql.len() <= SYNTAX_CACHE_MAX_BYTES {
            if let Some(previous) = &self.syntax_tree {
                let previous_normalized =
                    normalize_for_parser(&self.source, self.placeholder_dialect);
                let mut edited = previous.clone();
                edited.edit(&minimal_input_edit(&previous_normalized, &normalized));
                self.parser.parse(&normalized, Some(&edited))
            } else {
                self.parser.parse(&normalized, None)
            }
        } else {
            self.parser.parse(&normalized, None)
        };
        self.remember_syntax_tree(sql, tree.as_ref());
        tree
    }

    pub(super) fn remember_syntax_tree(&mut self, sql: &str, tree: Option<&Tree>) {
        self.syntax_tree = tree
            .filter(|tree| {
                sql.len() <= SYNTAX_CACHE_MAX_BYTES
                    && tree.root_node().descendant_count() <= SYNTAX_CACHE_MAX_NODES
            })
            .cloned();
        // Replace rather than clear an allocation left by a huge old document.
        self.source = sql.to_owned();
        self.source_positions = crate::position::SourcePositions::new(sql);
    }
}
