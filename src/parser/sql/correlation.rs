//! Correlation edges constrain row lookup independently of catalog metadata.
use super::query_scopes::query_openings;
use super::types::{RelationScope, SqlParser};
use tower_lsp::lsp_types::Position;
use tree_sitter::Tree;

const MAX_CORRELATION_SCOPES: usize = 32;
#[derive(Clone, Copy)]
enum Edge {
    Scalar,
    Isolated,
    Lateral,
}

impl SqlParser {
    /// Input positions are byte columns, as with other parser scope APIs.
    pub fn relation_scopes_at_position(
        &self,
        tree: &Tree,
        source: &str,
        position: Position,
    ) -> Vec<RelationScope> {
        let cursor = Self::byte_offset_for_position(source, position).min(source.len());
        let mask = Self::mask_sql_noise(source);
        let openings = query_openings(&mask[..cursor]);
        if openings.len() >= MAX_CORRELATION_SCOPES {
            return Vec::new();
        }
        let start = openings.last().map_or_else(
            || Self::previous_statement_start(&mask, cursor),
            |open| open + 1,
        );
        let mut result = vec![self
            .join_operand_scope(tree, source, position, start)
            .unwrap_or_else(|| {
                let scope = Self::completion_scope_with_mask(source, &mask, position);
                RelationScope {
                    joined: None,
                    occurrences: None,
                    start,
                    relations: Self::row_sources_in_searchable(&scope.source, &scope.searchable),
                    aliases: Self::extract_relation_alias_entries_in_searchable(
                        &scope.source,
                        &scope.searchable,
                    ),
                }
            })];
        for (index, open) in openings.iter().enumerate().rev() {
            let edge = correlation_edge(tree, &mask, *open);
            if matches!(edge, Edge::Isolated) {
                continue;
            }
            let start = index.checked_sub(1).map_or_else(
                || Self::previous_statement_start(&mask, *open),
                |parent| openings[parent] + 1,
            );
            let position = crate::position::byte_position_at_end(&source[..*open]);
            if matches!(edge, Edge::Lateral) {
                if let Some(scope) = self.lateral_source_scope(tree, source, *open, start) {
                    result.push(scope);
                    continue;
                }
            }
            if matches!(edge, Edge::Scalar) {
                if let Some(scope) = self.join_operand_scope(tree, source, position, start) {
                    result.push(scope);
                    continue;
                }
            }
            let mut scope = Self::completion_scope_with_mask(source, &mask, position);
            if matches!(edge, Edge::Lateral) {
                let mut introducer = mask[..*open].trim_end().len();
                while introducer > 0 && mask.as_bytes()[introducer - 1].is_ascii_alphabetic() {
                    introducer -= 1;
                }
                let end = introducer.saturating_sub(start).min(scope.source.len());
                scope.source.truncate(end);
                scope.searchable.truncate(end);
            }
            result.push(RelationScope {
                joined: None,
                occurrences: None,
                start,
                relations: Self::row_sources_in_searchable(&scope.source, &scope.searchable),
                aliases: Self::extract_relation_alias_entries_in_searchable(
                    &scope.source,
                    &scope.searchable,
                ),
            });
        }
        result
    }
}

fn correlation_edge(tree: &Tree, mask: &str, open: usize) -> Edge {
    let prefix = mask[..open].trim_end();
    let words: Vec<_> = prefix
        .rsplit(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|word| !word.is_empty())
        .take(5)
        .collect();
    let word = words.first().copied().unwrap_or("");
    if word.eq_ignore_ascii_case("APPLY") {
        return Edge::Lateral;
    }
    if word.eq_ignore_ascii_case("LATERAL") {
        return if words
            .iter()
            .take(4)
            .any(|word| word.eq_ignore_ascii_case("RIGHT") || word.eq_ignore_ascii_case("FULL"))
        {
            Edge::Isolated
        } else {
            Edge::Lateral
        };
    }
    let mut node = tree.root_node().descendant_for_byte_range(open, open + 1);
    while let Some(current) = node {
        if current.kind() == "subquery" && current.start_byte() == open {
            return match current.parent().map(|parent| parent.kind()) {
                Some("lateral_join" | "lateral_cross_join") => Edge::Lateral,
                Some("relation") => Edge::Isolated,
                _ => Edge::Scalar,
            };
        }
        if current.kind() == "cte" {
            return Edge::Isolated;
        }
        if current.start_byte() < open && matches!(current.kind(), "statement" | "program") {
            break;
        }
        node = current.parent();
    }
    // Error recovery may not yet have a subquery node. Only explicit source
    // introducers open a derived-table edge; functions/EXISTS remain scalar.
    if ["FROM", "JOIN", "AS", "MATERIALIZED"]
        .iter()
        .any(|candidate| word.eq_ignore_ascii_case(candidate))
    {
        return Edge::Isolated;
    }
    if prefix.ends_with(',') {
        let (start, _) = SqlParser::innermost_query_scope_start(prefix);
        let current =
            SqlParser::mask_nested_parenthesized_regions(&prefix[start..]).to_ascii_uppercase();
        let latest = [
            "SELECT", "FROM", "JOIN", "WHERE", "ON", "GROUP", "HAVING", "ORDER", "SET", "VALUES",
        ]
        .iter()
        .filter_map(|word| {
            let mut cursor = 0;
            let mut last = None;
            while let Some(at) = SqlParser::next_keyword_position(&current, word, cursor) {
                last = Some(at);
                cursor = at + word.len();
            }
            last.map(|at| (at, *word))
        })
        .max_by_key(|(at, _)| *at);
        if latest.is_some_and(|(_, word)| matches!(word, "FROM" | "JOIN")) {
            return Edge::Isolated;
        }
    }
    Edge::Scalar
}
