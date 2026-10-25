use super::super::diagnostics::{document_diagnostics, FALLBACK_PARSE_COUNT};
use super::*;
use crate::{
    dialect::Dialect,
    dialects::{ClickHouseDialect, HiveDialect, MysqlDialect, PostgresDialect},
    parser::ParseResult,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Hover, NumberOrString};
#[derive(Clone, Copy)]
enum Mode {
    Shared,
    NoTree,
    Stale,
}
struct CountingDialect {
    inner: PostgresDialect,
    mode: Mode,
    name: &'static str,
    parse_calls: AtomicUsize,
    snapshot_calls: AtomicUsize,
}
impl CountingDialect {
    fn new(mode: Mode, name: &'static str) -> Self {
        Self {
            inner: PostgresDialect::new(),
            mode,
            name,
            parse_calls: AtomicUsize::new(0),
            snapshot_calls: AtomicUsize::new(0),
        }
    }
}
fn marker(text: &str) -> Diagnostic {
    Diagnostic {
        message: text.into(),
        severity: Some(DiagnosticSeverity::HINT),
        ..Default::default()
    }
}
#[async_trait::async_trait]
impl Dialect for CountingDialect {
    fn name(&self) -> &str {
        self.name
    }
    async fn parse(&self, text: &str, schema: Option<&Schema>) -> Vec<Diagnostic> {
        self.parse_calls.fetch_add(1, Ordering::SeqCst);
        let mut d = self.inner.parse(text, schema).await;
        d.push(marker("fresh fallback"));
        d
    }
    async fn parse_with_syntax(&self, text: &str, schema: Option<&Schema>) -> ParseResult {
        self.snapshot_calls.fetch_add(1, Ordering::SeqCst);
        let source = if matches!(self.mode, Mode::Stale) {
            "SELECT (1+);"
        } else {
            text
        };
        let mut result = self.inner.parse_with_syntax(source, schema).await;
        if matches!(self.mode, Mode::NoTree) {
            result.tree = None;
        }
        result.diagnostics.push(marker("snapshot validation"));
        result
    }
    async fn completion(&self, _: &str, _: Position, _: Option<&Schema>) -> Vec<CompletionItem> {
        Vec::new()
    }
    async fn hover(&self, _: &str, _: Position, _: Option<&Schema>) -> Option<Hover> {
        None
    }
    async fn goto_definition(&self, _: &str, _: Position, _: Option<&Schema>) -> Option<Location> {
        None
    }
    async fn references(&self, _: &str, _: Position, _: Option<&Schema>) -> Vec<Location> {
        Vec::new()
    }
    async fn format(&self, text: &str) -> String {
        text.into()
    }
    async fn validate(&self, text: &str, schema: Option<&Schema>) -> Vec<Diagnostic> {
        self.parse(text, schema).await
    }
}
fn count(items: &[Diagnostic], code: &str) -> usize {
    items
        .iter()
        .filter(|d| d.code == Some(NumberOrString::String(code.into())))
        .count()
}
fn fallback_count() -> usize {
    FALLBACK_PARSE_COUNT.with(|n| n.get())
}
fn reset() {
    FALLBACK_PARSE_COUNT.with(|n| n.set(0));
}
#[tokio::test]
async fn shared_syntax_document_path_consumes_snapshot_without_secondary_parse() {
    let d = CountingDialect::new(Mode::Shared, "postgres");
    reset();
    let result = document_diagnostics(
        &d,
        "UPDATE users SET id=(SELECT id FROM users WHERE id=1);",
        None,
    )
    .await;
    assert_eq!(count(&result, "OXIDE001"), 1);
    assert!(result.iter().any(|d| d.message == "snapshot validation"));
    assert_eq!(d.snapshot_calls.load(Ordering::SeqCst), 1);
    assert_eq!(d.parse_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fallback_count(), 0);
}
#[tokio::test]
async fn shared_syntax_missing_tree_preserves_validation_and_fallback() {
    let d = CountingDialect::new(Mode::NoTree, "postgres");
    reset();
    let result = document_diagnostics(&d, "UPDATE users SET id=1;", None).await;
    assert_eq!(count(&result, "OXIDE001"), 1);
    assert!(result.iter().any(|d| d.message == "snapshot validation"));
    assert_eq!(d.snapshot_calls.load(Ordering::SeqCst), 1);
    assert_eq!(d.parse_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fallback_count(), 1);
}
#[tokio::test]
async fn shared_syntax_stale_source_rejected_before_diagnostics_or_ranges() {
    let d = CountingDialect::new(Mode::Stale, "postgres");
    reset();
    let result = document_diagnostics(&d, "/*😀*/ DELETE FROM users;", None).await;
    assert_eq!(count(&result, "OXIDE001"), 1);
    assert!(result.iter().any(|d| d.message == "fresh fallback"));
    assert!(!result.iter().any(
        |d| d.message == "snapshot validation" || d.severity == Some(DiagnosticSeverity::ERROR)
    ));
    assert_eq!(d.snapshot_calls.load(Ordering::SeqCst), 1);
    assert_eq!(d.parse_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fallback_count(), 1);
}
#[tokio::test]
async fn shared_syntax_non_sql_dialect_keeps_existing_path() {
    let d = CountingDialect::new(Mode::Shared, "mongodb");
    reset();
    let result = document_diagnostics(&d, "UPDATE users SET id=1;", None).await;
    assert_eq!(count(&result, "OXIDE001"), 0);
    assert_eq!(d.snapshot_calls.load(Ordering::SeqCst), 0);
    assert_eq!(d.parse_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fallback_count(), 0);
}
fn profiles() -> Vec<Box<dyn Dialect>> {
    vec![
        Box::new(PostgresDialect::new()),
        Box::new(MysqlDialect::new()),
        Box::new(PostgresDialect::sqlserver()),
        Box::new(ClickHouseDialect::new()),
        Box::new(HiveDialect::new()),
    ]
}
#[tokio::test]
async fn shared_syntax_real_profiles_keep_full_diagnostics_and_original_source() {
    for d in profiles() {
        for text in [
            "/*😀*/\r\nUPDATE users SET id=(SELECT id FROM users WHERE id=1);",
            "SELECT 1=ANY(+); SELECT a.id FROM a JOIN b\nWHERE a.id>0;",
        ] {
            let snapshot = d.parse_with_syntax(text, None).await;
            assert_eq!(snapshot.source, text);
            assert!(snapshot.tree.is_some(), "{}", d.name());
            assert_eq!(snapshot.diagnostics, d.parse(text, None).await);
        }
    }
}
#[tokio::test]
async fn shared_syntax_owned_snapshot_survives_another_document_parse() {
    let d = PostgresDialect::new();
    let source = "/*😀*/ WITH q AS(DELETE FROM users RETURNING id) SELECT id FROM q;";
    let first = d.parse_with_syntax(source, None).await;
    let shape = first.tree.as_ref().unwrap().root_node().to_sexp();
    let next = d.parse_with_syntax("SELECT 1;", None).await;
    assert_eq!(next.source, "SELECT 1;");
    assert_eq!(first.source, source);
    assert_eq!(first.tree.as_ref().unwrap().root_node().to_sexp(), shape);
    assert_eq!(
        count(&document_diagnostics(&d, source, None).await, "OXIDE001"),
        1
    );
    assert_eq!(
        count(
            &document_diagnostics(&d, "SELECT 1;", None).await,
            "OXIDE001"
        ),
        0
    );
}
#[tokio::test]
async fn shared_syntax_large_uncacheable_tree_is_shared_per_request() {
    let d = CountingDialect::new(Mode::Shared, "postgres");
    let source = format!(
        "/*{}*/\r\n{}",
        "padding ".repeat(40000),
        "UPDATE users SET id=1; ".repeat(4097)
    );
    let snapshot = d.inner.parse_with_syntax(&source, None).await;
    assert!(source.len() > 256 * 1024);
    assert!(
        snapshot
            .tree
            .as_ref()
            .unwrap()
            .root_node()
            .descendant_count()
            > 32768
    );
    reset();
    let result = document_diagnostics(&d, &source, None).await;
    assert_eq!(count(&result, "OXIDE001"), 4097);
    assert_eq!(fallback_count(), 0);
    let mut legacy = d.inner.parse(&source, None).await;
    legacy.push(marker("snapshot validation"));
    legacy.extend(sql_inspection_diagnostics(&source, None, "postgres"));
    assert_eq!(result, legacy);
}
#[tokio::test]
async fn shared_syntax_concurrent_document_requests_keep_ownership() {
    let d = Arc::new(CountingDialect::new(Mode::Shared, "postgres"));
    reset();
    let (a, b) = tokio::join!(
        document_diagnostics(
            &*d,
            "WITH q AS(DELETE FROM users RETURNING id) SELECT id FROM q;",
            None
        ),
        document_diagnostics(&*d, "UPDATE users SET id=1 WHERE id=2;", None)
    );
    assert_eq!(count(&a, "OXIDE001"), 1);
    assert_eq!(count(&b, "OXIDE001"), 0);
    assert_eq!(d.snapshot_calls.load(Ordering::SeqCst), 2);
    assert_eq!(d.parse_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fallback_count(), 0);
}
#[tokio::test]
async fn shared_syntax_profile_normalization_preserves_every_diagnostic() {
    for (d, source) in profiles().into_iter().zip([
        "UPDATE \"订单\" SET \"值\"=(SELECT id FROM users WHERE id=1);",
        "UPDATE `订单` SET `值`=(SELECT id FROM users WHERE id=1);",
        "CREATE PROC [app].[p] AS BEGIN UPDATE [订单] SET [值]=(SELECT 1 WHERE 1=1); END;",
        "// 隐藏 JOIN;\nSELECT 'a;b',a.id FROM users a JOIN users b\nWHERE a.id>0;",
        "SELECT a.id FROM users a JOIN users b\tWHERE a.id>0;",
    ]) {
        let mut legacy = d.parse(source, None).await;
        legacy.extend(sql_inspection_diagnostics(source, None, d.name()));
        reset();
        let shared = document_diagnostics(&*d, source, None).await;
        assert_eq!(shared, legacy, "{}", d.name());
        assert_eq!(fallback_count(), 0, "{}", d.name());
    }
}
