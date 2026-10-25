//! Vendor gating uses actual syntax nodes, never literal/comment spellings.
use crate::parser::SqlParser;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Range};
use tree_sitter::Tree;
pub(super) fn diagnostics(source: &str, profile: &str, tree: Option<&Tree>) -> Vec<Diagnostic> {
    if !matches!(
        profile,
        "postgres" | "mysql" | "sqlserver" | "sqlite" | "duckdb" | "clickhouse"
    ) || source.len() > 512 * 1024
        || !source
            .as_bytes()
            .windows(7)
            .any(|s| s[0].eq_ignore_ascii_case(&b'l') && s.eq_ignore_ascii_case(b"LATERAL"))
    {
        return Vec::new();
    }
    let fallback = tree
        .is_none()
        .then(|| SqlParser::new().parse_syntax_tree(source))
        .flatten();
    let Some(tree) = tree.or(fallback.as_ref()) else {
        return Vec::new();
    };
    SqlParser::lateral_views(tree, source)
        .into_iter()
        .map(|view| Diagnostic {
            range: Range::new(
                crate::position::lsp_position_at_end(&source[..view.range.start]),
                crate::position::lsp_position_at_end(&source[..view.range.start + 7]),
            ),
            severity: Some(DiagnosticSeverity::ERROR),
            code: Some(NumberOrString::String("UNSUPPORTED_DIALECT_SYNTAX".into())),
            source: Some("sql-dialect".into()),
            message: format!("LATERAL VIEW is not supported by the {profile} profile"),
            ..Default::default()
        })
        .collect()
}
