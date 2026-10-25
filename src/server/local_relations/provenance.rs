//! Source provenance retains a single original tree while inference descends
//! through slices. Only visible outer bindings are imported into a projection.
use crate::dialects::common::ColumnScopes;
use crate::{parser::SqlParser, placeholder::SqlPlaceholderDialect, schema::Schema};
use tree_sitter::Tree;

#[derive(Clone, Copy)]
pub(super) struct QueryOrigin<'a> {
    source: &'a str,
    tree: &'a Tree,
}
pub(super) struct ParsedOrigin<'a> {
    source: &'a str,
    tree: Option<Tree>,
}
impl<'a> ParsedOrigin<'a> {
    pub(super) fn for_comparison(
        source: &'a str,
        dialect: Option<&str>,
        operator: std::ops::Range<usize>,
    ) -> Self {
        if source.len() > super::LOCAL_RELATION_SCAN_MAX_BYTES
            || operator.end > source.len()
            || operator.end <= operator.start
        {
            return Self { source, tree: None };
        }
        let mut parsed = Self::new(source, dialect);
        if parsed
            .tree
            .as_ref()
            .and_then(|tree| comparison_left_range(tree, operator.start))
            .is_some()
        {
            return parsed;
        }
        if !source
            .get(operator.start..operator.start + 2)
            .is_some_and(|value| value.eq_ignore_ascii_case("IS"))
        {
            return parsed;
        }
        let mut recovery = source.as_bytes().to_vec();
        recovery[operator.start..operator.start + 2].copy_from_slice(b"=0");
        if operator.end > operator.start + 2 {
            if !source
                .get(operator.end - 3..operator.end)
                .is_some_and(|value| value.eq_ignore_ascii_case("NOT"))
            {
                return parsed;
            }
            recovery[operator.end - 3..operator.end].copy_from_slice(b"   ");
        }
        let Ok(recovery) = String::from_utf8(recovery) else {
            return parsed;
        };
        let mut parser =
            SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::for_name(dialect));
        parsed.tree = parser.parse_syntax_tree(&recovery);
        parsed
    }
    pub(super) fn for_completion(source: &'a str, dialect: Option<&str>, cursor: usize) -> Self {
        let mut parser =
            SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::for_name(dialect));
        Self {
            source,
            tree: parser.parse_completion_tree(
                source,
                crate::position::byte_position_at_end(&source[..cursor]),
            ),
        }
    }
    pub(super) fn qualify_keyword_at(&self, offset: usize) -> bool {
        self.tree
            .as_ref()
            .and_then(|tree| {
                tree.root_node()
                    .descendant_for_byte_range(offset, offset + 7)
            })
            .is_some_and(|node| node.kind() == "keyword_qualify")
    }
    pub(super) fn new(source: &'a str, dialect: Option<&str>) -> Self {
        let dialect = SqlPlaceholderDialect::for_name(dialect);
        let mut parser = SqlParser::new_with_placeholder_dialect(dialect);
        Self {
            source,
            tree: parser.parse_syntax_tree(source),
        }
    }
    pub(super) fn view(&self) -> Option<QueryOrigin<'_>> {
        self.tree.as_ref().map(|tree| QueryOrigin {
            source: self.source,
            tree,
        })
    }
    pub(super) fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
    }
}
pub(super) fn comparison_left_range(
    tree: &Tree,
    operator: usize,
) -> Option<std::ops::Range<usize>> {
    comparison_ranges(tree, operator).map(|ranges| ranges.0)
}
pub(super) fn comparison_ranges(
    tree: &Tree,
    operator: usize,
) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let end = operator.checked_add(1)?;
    let mut node = tree.root_node().descendant_for_byte_range(operator, end)?;
    while let Some(parent) = node.parent() {
        if parent
            .child_by_field_name("operator")
            .is_some_and(|value| value.start_byte() <= operator && value.end_byte() >= end)
        {
            let parent = if parent.child_by_field_name("operator")?.kind() == "keyword_escape" {
                let predicate = parent.parent()?;
                (predicate.child_by_field_name("right")?.id() == parent.id()).then_some(())?;
                predicate
            } else {
                parent
            };
            return Some((
                parent.child_by_field_name("left")?.byte_range(),
                parent.child_by_field_name("operator")?.byte_range(),
            ));
        }
        node = parent;
    }
    None
}
impl QueryOrigin<'_> {
    pub(super) fn exact_tree(&self, query: &str) -> Option<&Tree> {
        (query.as_ptr() == self.source.as_ptr() && query.len() == self.source.len())
            .then_some(self.tree)
    }
    pub(super) fn outer_columns<'a>(
        &self,
        query: &str,
        cursor: usize,
        parser: &SqlParser,
        schema: &'a Schema,
    ) -> Option<ColumnScopes<'a>> {
        // All inference children are slices of this immutable source. Checking
        // pointer identity and bounds avoids substring searches for repeated SQL.
        let start = query
            .as_ptr()
            .addr()
            .checked_sub(self.source.as_ptr().addr())?;
        let end = start.checked_add(query.len())?;
        let slice = self.source.get(start..end)?;
        if slice.as_ptr() != query.as_ptr() || cursor > query.len() {
            return None;
        }
        let position = crate::position::byte_position_at_end(&self.source[..start + cursor]);
        Some(ColumnScopes::new(
            parser,
            self.tree,
            self.source,
            position,
            schema,
            1,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Column, SchemaId, Table};
    #[test]
    fn shared_projection_trees_require_exact_source_identity_and_keep_canonical_recovery_separate()
    {
        let source = String::from("SELECT id FROM orders");
        let parsed = ParsedOrigin::new(&source, Some("hive"));
        let origin = parsed.view().unwrap();
        assert!(origin.exact_tree(&source).is_some());
        assert!(origin.exact_tree(&source.clone()).is_none());
        assert!(origin.exact_tree(&source[..source.len() - 1]).is_none());
        let sql = "SELECT e. FROM orders o LATERAL VIEW explode(o.tags) e AS item";
        let at = sql.find("e.").unwrap() + 2;
        let mut parser = SqlParser::new();
        let original = parser.parse_syntax_tree(sql).unwrap().root_node().to_sexp();
        let tree = parser
            .parse_completion_tree(sql, crate::position::byte_position_at_end(&sql[..at]))
            .unwrap();
        assert_eq!(SqlParser::lateral_views(&tree, sql).len(), 1);
        assert_eq!(
            parser.parse_syntax_tree(sql).unwrap().root_node().to_sexp(),
            original
        );
    }
    #[test]
    fn qualify_recovery_retains_a_real_keyword_without_reclassifying_aliases() {
        for sql in [
            "SELECT row_number() OVER() AS rn FROM orders QUALIFY ",
            "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1 AND (",
            "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1",
        ] {
            let origin = ParsedOrigin::new(sql, Some("clickhouse"));
            assert!(
                origin.qualify_keyword_at(sql.find("QUALIFY").unwrap()),
                "{sql}: {:?}",
                origin.tree.as_ref().map(|t| t.root_node().to_sexp())
            );
        }
        let sql = "SELECT qualify AS value FROM orders";
        assert!(!ParsedOrigin::new(sql, Some("clickhouse"))
            .qualify_keyword_at(sql.find("qualify").unwrap()));
    }
    #[test]
    fn repeated_body_text_keeps_its_actual_scope_and_unicode_offsets() {
        let source="/* 🧭 */\r\nSELECT b.value FROM orders o JOIN(SELECT o.total AS value) a ON TRUE CROSS JOIN LATERAL(SELECT o.total AS value) b";
        let body = "SELECT o.total AS value";
        let first = source.find(body).unwrap();
        let second = source.rfind(body).unwrap();
        let schema = Schema {
            id: SchemaId::new(),
            database: "app".into(),
            server: None,
            namespace: None,
            catalog: None,
            server_version: None,
            source_uri: None,
            functions: vec![],
            tables: vec![Table {
                name: "orders".into(),
                columns: vec![Column {
                    name: "total".into(),
                    data_type: "decimal".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let parsed = ParsedOrigin::new(source, Some("postgres"));
        let origin = parsed.view().unwrap();
        let parser = SqlParser::new();
        let isolated = origin
            .outer_columns(&source[first..first + body.len()], 7, &parser, &schema)
            .unwrap();
        assert_eq!(isolated.column_type("total", Some("o")), None);
        let lateral = origin
            .outer_columns(&source[second..second + body.len()], 7, &parser, &schema)
            .unwrap();
        assert_eq!(
            lateral.column_type("total", Some("o")).as_deref(),
            Some("decimal")
        );
        let copied = body.to_owned();
        assert_ne!(copied.as_ptr(), source[first..].as_ptr());
        assert!(origin.outer_columns(&copied, 7, &parser, &schema).is_none());
    }
}
