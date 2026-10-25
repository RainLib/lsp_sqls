//! Opaque configuration provenance, echoed from the analyzed snapshot.
use tower_lsp::lsp_types::{notification::Notification, PublishDiagnosticsParams};

pub(super) fn parse_analysis_token(value: Option<&serde_json::Value>) -> Option<String> {
    let value = value?.as_str()?;
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    .then(|| value.to_owned())
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ContextDiagnosticsParams {
    #[serde(flatten)]
    pub(super) diagnostics: PublishDiagnosticsParams,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) oxide_analysis_token: Option<String>,
}
pub(super) enum ContextDiagnostics {}
impl Notification for ContextDiagnostics {
    type Params = ContextDiagnosticsParams;
    const METHOD: &'static str = "textDocument/publishDiagnostics";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn tokens_are_bounded_opaque_ascii() {
        for token in ["a", "session-123_config_456", &"x".repeat(128)] {
            assert_eq!(
                parse_analysis_token(Some(&json!(token))),
                Some(token.to_owned())
            );
        }
    }
    #[test]
    fn invalid_values_cannot_preserve_or_grant_provenance() {
        for value in [
            json!(""),
            json!("x".repeat(129)),
            json!("a b"),
            json!("列"),
            json!("SQL;"),
            json!(null),
            json!(1),
            json!({"token":"x"}),
        ] {
            assert_eq!(parse_analysis_token(Some(&value)), None);
        }
        assert_eq!(parse_analysis_token(None), None);
    }
    #[test]
    fn standard_notification_fields_remain_compatible() {
        let diagnostics = PublishDiagnosticsParams {
            uri: "file:///query.sql".parse().unwrap(),
            diagnostics: vec![],
            version: Some(7),
        };
        let value = serde_json::to_value(ContextDiagnosticsParams {
            diagnostics,
            oxide_analysis_token: Some("context-2".into()),
        })
        .unwrap();
        assert_eq!(value["oxideAnalysisToken"], "context-2");
        let standard: PublishDiagnosticsParams = serde_json::from_value(value).unwrap();
        assert_eq!(standard.version, Some(7));
        assert!(standard.diagnostics.is_empty());
    }
    #[test]
    fn legacy_configuration_has_no_fabricated_token() {
        let diagnostics = PublishDiagnosticsParams {
            uri: "file:///query.sql".parse().unwrap(),
            diagnostics: vec![],
            version: Some(7),
        };
        let value = serde_json::to_value(ContextDiagnosticsParams {
            diagnostics,
            oxide_analysis_token: None,
        })
        .unwrap();
        assert!(value.get("oxideAnalysisToken").is_none());
    }
}
