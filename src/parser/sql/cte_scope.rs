use super::types::SqlParser;
use std::collections::HashSet;

impl SqlParser {
    pub(super) fn skip_cte_materialization_hint(source: &str, index: usize) -> usize {
        let index = Self::skip_whitespace(source, index);
        if let Some(after_not) = Self::consume_word(source, index, "NOT") {
            if let Some(after_materialized) = Self::consume_word(source, after_not, "MATERIALIZED")
            {
                return after_materialized;
            }
        }

        Self::consume_word(source, index, "MATERIALIZED").unwrap_or(index)
    }

    pub(super) fn extract_cte_names(
        source: &str,
        searchable_source: &str,
        source_upper: &str,
    ) -> HashSet<String> {
        let mut names = HashSet::new();
        let mut search_pos = 0;

        while let Some(with_pos) = Self::next_keyword_position(source_upper, "WITH", search_pos) {
            let mut index = with_pos + "WITH".len();
            index = Self::consume_word(searchable_source, index, "RECURSIVE").unwrap_or(index);

            loop {
                index = Self::skip_whitespace(searchable_source, index);
                let Some((cte_name, after_name)) = Self::read_identifier_part(source, index) else {
                    break;
                };
                names.insert(Self::normalize_identifier(&cte_name));
                index = Self::skip_whitespace(searchable_source, after_name);

                if searchable_source[index..].starts_with('(') {
                    index = Self::skip_parenthesized_region(searchable_source, index);
                }

                let Some(after_as) = Self::consume_word(searchable_source, index, "AS") else {
                    break;
                };
                index = Self::skip_cte_materialization_hint(searchable_source, after_as);
                index = Self::skip_whitespace(searchable_source, index);
                if !searchable_source[index..].starts_with('(') {
                    break;
                }

                index = Self::skip_parenthesized_region(searchable_source, index);
                index = Self::skip_whitespace(searchable_source, index);
                if searchable_source[index..].starts_with(',') {
                    index += 1;
                    continue;
                }
                break;
            }

            search_pos = index.max(with_pos + "WITH".len());
        }

        names
    }

    pub(super) fn is_cte_reference(table_name: &str, cte_names: &HashSet<String>) -> bool {
        let normalized = Self::normalize_identifier(table_name);
        let parts = Self::identifier_parts(&normalized);
        parts.len() == 1 && cte_names.contains(&normalized)
    }
}
