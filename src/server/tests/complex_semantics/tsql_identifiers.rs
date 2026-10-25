use super::quoted_names::typed;
use super::*;
#[tokio::test]
async fn bracket_members_keep_hover_and_metadata_types() {
    for sql in [
        "SELECT [u].[na<cursor>me] FROM [users] AS [u]",
        "SELECT [o.x].[display]]na<cursor>me] FROM [users] AS [o.x]",
        "SELECT [u].[white sp<cursor>ace] FROM [users] [u]",
    ] {
        let name = if sql.contains("display") {
            "display]name"
        } else if sql.contains("white") {
            "white space"
        } else {
            "name"
        };
        typed("sqlserver", sql, name, "text").await;
    }
}
#[tokio::test]
async fn bracket_outputs_flow_through_ctes_derived_and_apply() {
    for (sql,ty) in [
        ("WITH [c.x] AS (SELECT [u].[name] AS [result] FROM [users] [u]) SELECT [c.x].[re<cursor>sult] FROM [c.x]","text"),
        ("SELECT [d.x].[re<cursor>sult] FROM (SELECT [u].[id] AS [result] FROM [users] [u]) [d.x]","bigint"),
        ("SELECT [d.x].[re<cursor>sult] FROM [users] [u] CROSS APPLY(SELECT [u].[name] AS [result]) [d.x]","text"),
    ] { typed("sqlserver",sql,"result",ty).await; }
}
#[tokio::test]
async fn valid_bracket_sql_has_no_parse_error_but_real_errors_remain() {
    let harness = configured("sqlserver", "SELECT [u].[na<cursor>me] FROM [users] [u]");
    let diagnostics = harness
        .service
        .inner()
        .diagnostic(
            serde_json::from_value(serde_json::json!({"textDocument":{"uri":harness.uri}}))
                .unwrap(),
        )
        .await
        .unwrap();
    let diagnostics = serde_json::to_value(diagnostics).unwrap();
    assert!(
        diagnostics["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["severity"] != 1 && item["code"] != "PARSE_ERROR"),
        "{diagnostics}"
    );
    let provider = harness
        .service
        .inner()
        .dialect_registry
        .get_by_name("sqlserver")
        .unwrap();
    assert!(!provider
        .parse("SELECT (1 + ) FROM [users]", None)
        .await
        .is_empty());
}

#[tokio::test]
async fn tsql_aliases_and_reserved_quoted_alias_keep_semantic_profile() {
    for dialect in ["sqlserver", "mssql", "tsql"] {
        typed(
            dialect,
            "SELECT [order].[na<cursor>me] FROM [users] AS [order]",
            "name",
            "text",
        )
        .await;
    }
}
#[tokio::test]
async fn tsql_nested_comments_do_not_offer_semantic_candidates() {
    let harness = configured(
        "sqlserver",
        "SELECT /* outer /* inner */ [u].[na<cursor>me] */ [name] FROM [users] [u]",
    );
    assert!(complete_harness(&harness).await.is_empty());
}

#[tokio::test]
async fn quoted_reserved_aliases_are_valid_in_multiple_profiles() {
    for (dialect, quote) in [
        ("postgres", "\"order\""),
        ("mysql", "`order`"),
        ("sqlserver", "[order]"),
    ] {
        typed(
            dialect,
            &format!("SELECT {quote}.na<cursor>me FROM users AS {quote}"),
            "name",
            "text",
        )
        .await;
        typed(
            dialect,
            &format!(
                "SELECT {quote}.re<cursor>sult FROM (SELECT name AS result FROM users) {quote}"
            ),
            "result",
            "text",
        )
        .await;
    }
}
