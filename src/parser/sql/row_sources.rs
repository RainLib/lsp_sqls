use super::types::{AstNode, SqlParser};
use tower_lsp::lsp_types::Position;
use tree_sitter::Node;
use tree_sitter::Tree;

impl SqlParser {
    /// 提取SQL中引用的表名（从FROM和JOIN子句）
    pub fn extract_referenced_tables(&self, _tree: &Tree, source: &str) -> Vec<String> {
        Self::extract_referenced_tables_from_source(source)
    }

    pub fn extract_common_table_expressions(&self, source: &str) -> Vec<String> {
        let searchable_source = Self::mask_sql_noise(source);
        let source_upper = searchable_source.to_ascii_uppercase();
        let mut names = Self::extract_cte_names(source, &searchable_source, &source_upper)
            .into_iter()
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    pub fn extract_referenced_tables_at_position(
        &self,
        tree: &Tree,
        source: &str,
        position: Position,
    ) -> Vec<String> {
        if self.uses_joined_scope(tree, source, position) {
            let ctes = self.extract_common_table_expressions(source);
            return self
                .relation_scope_for_member(tree, source, position)
                .map(|s| {
                    s.relations
                        .into_iter()
                        .filter(|r| !ctes.iter().any(|c| c.eq_ignore_ascii_case(r)))
                        .collect()
                })
                .unwrap_or_default();
        }
        let scope = Self::completion_scope_analysis(source, position);
        Self::extract_referenced_tables_in_searchable(&scope.source, &scope.searchable)
    }

    /// Extracts row-source references in the active query, including CTE
    /// names. Schema inference deliberately excludes CTEs, but editor
    /// intentions need them because the position schema contains their local
    /// projected columns.
    pub fn extract_row_sources_at_position(
        &self,
        tree: &Tree,
        source: &str,
        position: Position,
    ) -> Vec<String> {
        if self.uses_joined_scope(tree, source, position) {
            return self
                .relation_scope_for_member(tree, source, position)
                .map(|s| s.relations)
                .unwrap_or_default();
        }
        let scope = Self::completion_scope_analysis(source, position);
        Self::row_sources_in_searchable(&scope.source, &scope.searchable)
    }

    pub(super) fn row_sources_in_searchable(source: &str, searchable: &str) -> Vec<String> {
        let source_upper = searchable.to_ascii_uppercase();
        let mut sources = Vec::new();
        for keyword in ["FROM", "JOIN", "APPLY", "UPDATE", "INTO", "TABLE", "VIEW"] {
            let mut search_position = 0;
            while let Some(position) =
                Self::next_keyword_position(&source_upper, keyword, search_position)
            {
                let after_keyword = position + keyword.len();
                if keyword == "VIEW"
                    && source_upper[..position].split_whitespace().next_back() == Some("LATERAL")
                {
                    search_position = after_keyword;
                    continue;
                }
                if let Some((relation, _)) =
                    Self::read_row_source_reference_after(source, searchable, after_keyword)
                {
                    Self::push_table_reference(&mut sources, &relation);
                }
                search_position = after_keyword;
            }
        }
        for relation_start in Self::comma_separated_relation_starts(&source_upper) {
            if let Some((relation, _)) =
                Self::read_row_source_reference_after(source, searchable, relation_start)
            {
                Self::push_table_reference(&mut sources, &relation);
            }
        }
        Self::extract_on_relation_references(source, &source_upper, &mut sources);
        sources
    }

    /// A derived relation is referenced by its alias, never by its hidden body.
    fn read_row_source_reference_after(
        source: &str,
        searchable: &str,
        index: usize,
    ) -> Option<(String, usize)> {
        if let Some(reference) = Self::read_relation_reference_after(source, index) {
            return Some(reference);
        }
        let open = Self::skip_relation_modifiers(source, index);
        if source.as_bytes().get(open) != Some(&b'(') {
            return None;
        }
        let close = Self::matching_paren_end(searchable, open)?;
        let (alias, _, end) =
            Self::read_relation_alias_with_span_after(source, searchable, close + 1)?;
        Some((Self::identifier_path_from_parts(&[alias]), end))
    }

    pub(super) fn extract_referenced_tables_from_source(source: &str) -> Vec<String> {
        let searchable_source = Self::mask_sql_noise(source);
        Self::extract_referenced_tables_in_searchable(source, &searchable_source)
    }

    fn extract_referenced_tables_in_searchable(
        source: &str,
        searchable_source: &str,
    ) -> Vec<String> {
        let mut tables = Vec::new();
        let source_upper = searchable_source.to_ascii_uppercase();
        let cte_names = Self::extract_cte_names(source, searchable_source, &source_upper);

        let keywords = ["FROM", "JOIN", "APPLY", "UPDATE", "INTO", "TABLE", "VIEW"];

        for keyword in keywords {
            let mut search_pos = 0;
            while let Some(abs_pos) =
                Self::next_keyword_position(&source_upper, keyword, search_pos)
            {
                let after_keyword = abs_pos + keyword.len();
                if keyword == "VIEW"
                    && source_upper[..abs_pos].split_whitespace().next_back() == Some("LATERAL")
                {
                    search_pos = after_keyword;
                    continue;
                }

                if let Some((table_name, _)) =
                    Self::read_relation_reference_after(source, after_keyword)
                {
                    if !Self::is_cte_reference(&table_name, &cte_names) {
                        Self::push_table_reference(&mut tables, &table_name);
                    }
                }

                search_pos = after_keyword;
            }
        }

        for relation_start in Self::comma_separated_relation_starts(&source_upper) {
            if let Some((table_name, _)) =
                Self::read_relation_reference_after(source, relation_start)
            {
                if !Self::is_cte_reference(&table_name, &cte_names) {
                    Self::push_table_reference(&mut tables, &table_name);
                }
            }
        }

        Self::extract_on_relation_references(source, &source_upper, &mut tables);

        tables
    }

    /// 将 Tree-sitter Node 转换为 AstNode
    pub fn node_to_ast_node(&self, node: Node, source: &str) -> AstNode {
        AstNode {
            node_type: node.kind().to_string(),
            position: self.node_range(node),
            text: self.node_text(node, source),
        }
    }
}
