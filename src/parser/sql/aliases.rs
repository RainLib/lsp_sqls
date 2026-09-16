use super::types::{RelationAlias, SqlParser};
use std::collections::HashMap;
use tower_lsp::lsp_types::Position;
use tree_sitter::Tree;

impl SqlParser {
    /// 提取表别名映射 (Alias -> Table Name)
    /// Uses text-based extraction for reliability
    pub fn extract_aliases(&self, _tree: &Tree, source: &str) -> HashMap<String, String> {
        Self::extract_aliases_from_source(source)
    }

    pub fn extract_aliases_at_position(
        &self,
        _tree: &Tree,
        source: &str,
        position: Position,
    ) -> HashMap<String, String> {
        Self::relation_aliases_at_position(source, position)
            .into_iter()
            .map(|alias| (alias.name, alias.relation))
            .collect()
    }

    /// Returns aliases from the innermost query visible at `position` while
    /// retaining their original SQL spelling for safe completion insertion.
    /// If a qualified reference targets an alias from a correlated outer
    /// query, only that requested outer alias is imported into the inner scope.
    pub fn relation_aliases_at_position(source: &str, position: Position) -> Vec<RelationAlias> {
        let scoped_source = Self::completion_scope_source(source, position);
        let mut aliases = Self::extract_relation_alias_entries_from_source(&scoped_source);
        let Some(qualifier) = Self::column_qualifier_before_position(source, position)
            .map(|value| Self::normalize_identifier(&value))
        else {
            return aliases;
        };
        if Self::resolve_relation_alias(&aliases, &qualifier).is_some() {
            return aliases;
        }

        // A qualified reference inside a correlated subquery may target an
        // alias from an enclosing SELECT. Import only the requested alias;
        // unqualified completion must remain isolated to the inner query.
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let searchable_prefix = Self::mask_sql_noise(
            source
                .get(..cursor_offset.min(source.len()))
                .unwrap_or(source),
        );
        let (_, mut scope_open) = Self::innermost_query_scope_start(&searchable_prefix);
        while let Some(open) = scope_open {
            let outer_position = crate::position::byte_position_at_end(&source[..open]);
            let outer_scope = Self::completion_scope_source(source, outer_position);
            let outer_aliases = Self::extract_relation_alias_entries_from_source(&outer_scope);
            if let Some(alias) = Self::resolve_relation_alias(&outer_aliases, &qualifier) {
                aliases.push(alias.clone());
                break;
            }
            let (_, next_scope_open) =
                Self::innermost_query_scope_start(&searchable_prefix[..open]);
            scope_open = next_scope_open;
        }
        aliases.sort_by(|left, right| left.name.cmp(&right.name));
        aliases
    }

    /// Resolve a visible relation alias. Exact names retain scope order; SQL's
    /// ordinary case-insensitive fallback is applied only to unquoted aliases.
    pub fn resolve_relation_alias<'a>(
        aliases: &'a [RelationAlias],
        qualifier: &str,
    ) -> Option<&'a RelationAlias> {
        let qualifier = Self::normalize_identifier(qualifier);
        if Self::identifier_parts(&qualifier).len() != 1 {
            return None;
        }

        aliases
            .iter()
            .find(|alias| alias.name == qualifier)
            .or_else(|| {
                aliases.iter().find(|alias| {
                    !Self::is_quoted_identifier_sql(&alias.sql)
                        && alias.name.eq_ignore_ascii_case(&qualifier)
                })
            })
    }

    pub(super) fn is_quoted_identifier_sql(value: &str) -> bool {
        let value = value.trim();
        matches!(
            value.as_bytes(),
            [b'"', .., b'"'] | [b'`', .., b'`'] | [b'[', .., b']']
        )
    }

    pub(super) fn extract_aliases_from_source(source: &str) -> HashMap<String, String> {
        Self::extract_relation_alias_entries_from_source(source)
            .into_iter()
            .map(|alias| (alias.name, alias.relation))
            .collect()
    }

    pub(super) fn extract_relation_alias_entries_from_source(source: &str) -> Vec<RelationAlias> {
        let mut aliases = HashMap::<String, RelationAlias>::new();
        let searchable_source = Self::mask_sql_noise(source);
        let source_upper = searchable_source.to_ascii_uppercase();

        // Pattern: FROM/JOIN/UPDATE table_name alias
        let keywords = ["FROM", "JOIN", "APPLY", "UPDATE"];

        for keyword in keywords {
            let mut search_pos = 0;
            while let Some(abs_pos) =
                Self::next_keyword_position(&source_upper, keyword, search_pos)
            {
                let after_keyword = abs_pos + keyword.len();

                if let Some((table_name, after_table)) =
                    Self::read_relation_reference_after(source, after_keyword)
                {
                    if let Some((alias, alias_start, alias_end)) =
                        Self::read_relation_alias_with_span_after(
                            source,
                            &searchable_source,
                            after_table,
                        )
                    {
                        let name = Self::normalize_identifier(&alias);
                        aliases.insert(
                            name.clone(),
                            RelationAlias {
                                name,
                                sql: source[alias_start..alias_end].to_string(),
                                relation: Self::normalize_relation_reference(&table_name),
                            },
                        );
                    }
                }

                search_pos = after_keyword;
            }
        }

        // FROM clauses may introduce additional row sources with commas,
        // including after a JOIN expression. Those sources have the same alias
        // semantics as the first FROM/JOIN relation.
        for relation_start in Self::comma_separated_relation_starts(&source_upper) {
            if let Some((table_name, after_table)) =
                Self::read_relation_reference_after(source, relation_start)
            {
                if let Some((alias, alias_start, alias_end)) =
                    Self::read_relation_alias_with_span_after(
                        source,
                        &searchable_source,
                        after_table,
                    )
                {
                    let name = Self::normalize_identifier(&alias);
                    aliases.insert(
                        name.clone(),
                        RelationAlias {
                            name,
                            sql: source[alias_start..alias_end].to_string(),
                            relation: Self::normalize_relation_reference(&table_name),
                        },
                    );
                }
            }
        }

        let mut aliases = aliases.into_values().collect::<Vec<_>>();
        aliases.sort_by(|left, right| left.name.cmp(&right.name));
        aliases
    }
}
