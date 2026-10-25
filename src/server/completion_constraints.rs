//! Complete keyword sets are explicit semantic constraints, not an inference
//! from a filtered, capped, empty or failed completion list.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionKeywordConstraints {
    pub slot: &'static str,
    pub allowed_keywords: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truth_tests: Option<bool>,
}
