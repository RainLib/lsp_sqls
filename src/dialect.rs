use crate::schema::Schema;
use async_trait::async_trait;
use tower_lsp::lsp_types::{
    CompletionContext as LspCompletionContext, CompletionItem, Diagnostic, Hover, Location,
    Position,
};

/// Request-owned metadata from the same SQL tree that produced the candidates.
/// None preserves legacy/custom dialect alias handling; Some(empty) is an
/// authoritative empty visible set, not a request to fall back to the catalog.
pub struct CompletionSnapshot {
    pub items: Vec<CompletionItem>,
    pub relation_aliases: Option<Vec<crate::parser::RelationAlias>>,
}
impl CompletionSnapshot {
    pub(crate) fn for_sql(
        parser: &crate::parser::SqlParser,
        tree: Option<&tree_sitter::Tree>,
        sql: &str,
        position: Position,
    ) -> Self {
        let relation_aliases = tree
            .filter(|tree| parser.uses_joined_scope(tree, sql, position))
            .map(|tree| {
                parser
                    .relation_scope_for_member(tree, sql, position)
                    .map(|s| s.aliases)
                    .unwrap_or_default()
            });
        Self {
            items: Vec::new(),
            relation_aliases,
        }
    }
}

/// SQL 方言抽象 trait
/// 所有 SQL 方言都需要实现这个 trait
#[async_trait]
#[allow(
    clippy::double_must_use,
    reason = "async_trait adds must_use to methods that already return must-use futures"
)]
pub trait Dialect: Send + Sync {
    /// 方言名称
    fn name(&self) -> &str;

    /// 解析 SQL 并返回诊断信息
    async fn parse(&self, sql: &str, schema: Option<&Schema>) -> Vec<Diagnostic>;

    /// Return validation and its exact-source SQL tree from one parse pass.
    /// The default keeps existing dialect implementations compatible. SQL
    /// providers may return their owned ParseResult after releasing the parser
    /// lock; consumers must reject a result for a different source document.
    async fn parse_with_syntax(
        &self,
        sql: &str,
        schema: Option<&Schema>,
    ) -> crate::parser::ParseResult {
        crate::parser::ParseResult {
            tree: None,
            diagnostics: self.parse(sql, schema).await,
            source: sql.to_owned(),
            success: false,
        }
    }

    /// 获取代码补全
    async fn completion(
        &self,
        sql: &str,
        position: Position,
        schema: Option<&Schema>,
    ) -> Vec<CompletionItem>;

    /// 获取代码补全，并保留 LSP 触发上下文（手动触发、触发字符、incomplete retry）。
    /// 默认回退到不带上下文的补全，方言可以按需覆盖。
    async fn completion_with_context(
        &self,
        sql: &str,
        position: Position,
        schema: Option<&Schema>,
        _context: Option<&LspCompletionContext>,
    ) -> Vec<CompletionItem> {
        self.completion(sql, position, schema).await
    }

    async fn completion_with_scope(
        &self,
        sql: &str,
        position: Position,
        schema: Option<&Schema>,
        context: Option<&LspCompletionContext>,
    ) -> CompletionSnapshot {
        CompletionSnapshot {
            items: self
                .completion_with_context(sql, position, schema, context)
                .await,
            relation_aliases: None,
        }
    }

    /// 获取悬停信息
    async fn hover(&self, sql: &str, position: Position, schema: Option<&Schema>) -> Option<Hover>;

    /// 跳转到定义
    async fn goto_definition(
        &self,
        sql: &str,
        position: Position,
        schema: Option<&Schema>,
    ) -> Option<Location>;

    /// 查找引用
    async fn references(
        &self,
        sql: &str,
        position: Position,
        schema: Option<&Schema>,
    ) -> Vec<Location>;

    /// 格式化 SQL
    async fn format(&self, sql: &str) -> String;

    /// 验证 SQL 语法
    async fn validate(&self, sql: &str, schema: Option<&Schema>) -> Vec<Diagnostic>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::{CompletionTriggerKind, DiagnosticSeverity, Range};

    struct TestDialect;

    #[async_trait]
    impl Dialect for TestDialect {
        fn name(&self) -> &str {
            "test"
        }

        async fn parse(&self, _sql: &str, _schema: Option<&Schema>) -> Vec<Diagnostic> {
            Vec::new()
        }

        async fn completion(
            &self,
            _sql: &str,
            _position: Position,
            _schema: Option<&Schema>,
        ) -> Vec<CompletionItem> {
            vec![CompletionItem {
                label: "fallback".to_string(),
                ..Default::default()
            }]
        }

        async fn hover(
            &self,
            _sql: &str,
            _position: Position,
            _schema: Option<&Schema>,
        ) -> Option<Hover> {
            None
        }

        async fn goto_definition(
            &self,
            _sql: &str,
            _position: Position,
            _schema: Option<&Schema>,
        ) -> Option<Location> {
            None
        }

        async fn references(
            &self,
            _sql: &str,
            _position: Position,
            _schema: Option<&Schema>,
        ) -> Vec<Location> {
            Vec::new()
        }

        async fn format(&self, sql: &str) -> String {
            sql.to_string()
        }

        async fn validate(&self, _sql: &str, _schema: Option<&Schema>) -> Vec<Diagnostic> {
            vec![Diagnostic {
                range: Range::default(),
                severity: Some(DiagnosticSeverity::HINT),
                message: "test".to_string(),
                ..Default::default()
            }]
        }
    }

    #[tokio::test]
    async fn default_completion_with_context_falls_back_to_completion() {
        let context = LspCompletionContext {
            trigger_kind: CompletionTriggerKind::TRIGGER_CHARACTER,
            trigger_character: Some(".".to_string()),
        };

        let items = TestDialect
            .completion_with_context(
                "SELECT u.",
                Position {
                    line: 0,
                    character: 9,
                },
                None,
                Some(&context),
            )
            .await;

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "fallback");
    }
    #[tokio::test]
    async fn shared_syntax_default_preserves_existing_dialect_validation() {
        let text = "UPDATE users SET id=1;";
        let parsed = TestDialect.parse_with_syntax(text, None).await;
        assert_eq!(parsed.source, text);
        assert!(parsed.tree.is_none());
        assert_eq!(parsed.diagnostics, TestDialect.parse(text, None).await);
    }

    #[tokio::test]
    async fn join_operands_completion_snapshot_default_preserves_custom_dialect_items() {
        let context = LspCompletionContext {
            trigger_kind: CompletionTriggerKind::TRIGGER_FOR_INCOMPLETE_COMPLETIONS,
            trigger_character: None,
        };
        let result = TestDialect
            .completion_with_scope("custom", Position::default(), None, Some(&context))
            .await;
        assert_eq!(
            result.items,
            TestDialect
                .completion_with_context("custom", Position::default(), None, Some(&context))
                .await
        );
        assert!(result.relation_aliases.is_none());
    }
}
