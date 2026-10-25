use super::*;
use serde_json::{json, Value};
use tower_lsp::LspService;
const URI: &str = "oxide://snapshot/current.mysql.sql";
fn configure(server: &SqlLspServer) {
    server.apply_configuration(json!({"oxideAnalysisToken":"confirmed","schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","functions":[{"name":"lower","routine_type":"function","parameters":[{"name":"value","data_type":"text","optional":false}],"return_type":"boolean"}],"tables":[]}],"fileSchemas":{(URI):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(URI):"postgres"},"completionPreferences":{"keywordCase":"upper"}}));
}
fn params(position: Position, patch: Value) -> CompletionSnapshotParams {
    let mut value = json!({"textDocument":{"uri":URI},"version":7,"dialect":"postgres","position":position,"oxideAnalysisToken":"confirmed"});
    for (key, patch_value) in patch.as_object().unwrap() {
        value[key] = patch_value.clone();
    }
    serde_json::from_value(value).unwrap()
}
#[tokio::test]
async fn completed_predicate_scalars_share_bound_logical_knowledge_without_borrowing_catalog() {
    let (service, _) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    configure(server);
    let text = "SELECT 1 WHERE LOWER('x') ";
    server.document_manager.update(URI.into(), text.into(), 7);
    let position = crate::position::lsp_position_at_end(text);
    let catalog = server
        .completion_snapshot(params(position, json!({})))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(catalog).unwrap()["logicalContinuation"],
        true
    );
    let local=server.document_completion_snapshot(serde_json::from_value(json!({"textDocument":{"uri":URI},"version":7,"dialect":"postgres","position":position,"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).unwrap()).await.unwrap();
    assert_eq!(
        serde_json::to_value(local).unwrap()["logicalContinuation"],
        false
    );
}
#[tokio::test]
async fn schema_free_comparison_values_use_local_inference_without_warm_catalog_authority() {
    let (service, _) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    server.apply_configuration(json!({"oxideAnalysisToken":"confirmed","defaultDialect":"postgres","schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"private","functions":[{"name":"private_fn","return_type":"integer","parameters":[]}],"tables":[]}],"fileSchemas":{},"fileDialects":{(URI):"postgres"}}));
    for (expression, expected) in [
        ("CAST(1 AS INTEGER)", Some("INTEGER")),
        ("STRPOS('a','b')", Some("integer")),
        ("private_fn()", None),
    ] {
        let text = format!("/*😀*/ SELECT {expression} = ");
        server.document_manager.update(URI.into(), text.clone(), 7);
        let position = crate::position::lsp_position_at_end(&text);
        let result = server
            .completion_snapshot(params(position, json!({})))
            .await
            .unwrap();
        assert_eq!(result.value_type.as_deref(), expected, "{expression}");
        let standard = server
            .completion_candidates_at(URI, &text, position, None)
            .await
            .unwrap();
        assert_eq!(standard.value_type.as_deref(), expected, "{expression}");
        if expected.is_some() {
            assert!(
                standard.items.iter().any(|item| item.label == "0"),
                "{expression}"
            );
            assert!(
                !standard.items.iter().any(|item| item.label == "TRUE"),
                "{expression}"
            );
        }
    }
}
#[tokio::test]
async fn snapshot_position_checks_accept_crlf_and_reject_newline_columns_consistently() {
    let (service, _) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    configure(server);
    let text = "/*😀*/\r\nSELECT CAST(1 AS INTEGER) = ";
    server.document_manager.update(URI.into(), text.into(), 7);
    for position in [
        Position::new(0, 2),
        Position::new(0, 4),
        Position::new(0, 6),
        Position::new(1, 0),
        Position::new(1, 28),
    ] {
        let mut local:super::super::document_completions::DocumentCompletionParams=serde_json::from_value(json!({"textDocument":{"uri":URI},"version":7,"dialect":"postgres","position":position,"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).unwrap();
        local.position = position;
        assert!(
            server.document_completion_snapshot(local).await.is_ok(),
            "valid {position:?}"
        );
        assert!(
            server
                .completion_snapshot(params(position, json!({})))
                .await
                .is_ok(),
            "valid catalog {position:?}"
        );
    }
    for position in [
        Position::new(0, 3),
        Position::new(0, 7),
        Position::new(1, 29),
        Position::new(2, 0),
    ] {
        assert!(
            server
                .completion_snapshot(params(position, json!({})))
                .await
                .is_err(),
            "invalid {position:?}"
        );
    }
}
#[tokio::test]
async fn live_comparison_value_types_do_not_leak_into_document_only_inference() {
    let (service, _) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    configure(server);
    let text = "SELECT LOWER('x') = ";
    server.document_manager.update(URI.into(), text.into(), 7);
    let position = crate::position::lsp_position_at_end(text);
    let catalog = server
        .completion_snapshot(params(position, json!({})))
        .await
        .unwrap();
    assert_eq!(catalog.value_type.as_deref(), Some("boolean"));
    assert!(catalog.items.iter().any(|item| item.label == "TRUE"));
    let local=server.document_completion_snapshot(serde_json::from_value(json!({"textDocument":{"uri":URI},"version":7,"dialect":"postgres","position":position,"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).unwrap()).await.unwrap();
    assert_eq!(local.value_type.as_deref(), Some("text"));
    assert!(local.items.iter().any(|item| item.label == "''"));
    assert!(!local.items.iter().any(|item| item.label == "TRUE"));
    assert!(local.items.iter().all(|item| item.data.is_none()));
}
#[tokio::test]
async fn full_constraints_are_not_the_prefix_filtered_candidate_list() {
    let (service, _) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    configure(server);
    let prefix = "/*😀*/ SELECT CAST(1 AS INTEGER) IS N";
    let text = format!("{prefix}Suffix;");
    server.document_manager.update(URI.into(), text, 7);
    let result = server
        .completion_snapshot(params(
            crate::position::lsp_position_at_end(prefix),
            json!({}),
        ))
        .await
        .unwrap();
    assert!(result.items.iter().all(|item| item.label.starts_with('N')));
    let constraint = result.constraints.unwrap();
    assert_eq!(constraint.slot, "is");
    assert!(constraint
        .allowed_keywords
        .iter()
        .any(|word| word == "DISTINCT FROM"));
    assert!(!constraint
        .allowed_keywords
        .iter()
        .any(|word| word == "TRUE"));
}
#[tokio::test]
async fn live_return_constraints_and_document_only_knowledge_remain_separate() {
    let (service, _) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    configure(server);
    let text = "SELECT LOWER('x') IS ";
    server.document_manager.update(URI.into(), text.into(), 7);
    let position = crate::position::lsp_position_at_end(text);
    let catalog = server
        .completion_snapshot(params(position, json!({})))
        .await
        .unwrap();
    assert_eq!(
        catalog.constraints.as_ref().unwrap().truth_tests,
        Some(true)
    );
    assert!(catalog
        .constraints
        .unwrap()
        .allowed_keywords
        .iter()
        .any(|word| word == "TRUE"));
    let local=server.document_completion_snapshot(serde_json::from_value(json!({"textDocument":{"uri":URI},"version":7,"dialect":"postgres","position":position,"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).unwrap()).await.unwrap();
    assert_eq!(local.constraints.as_ref().unwrap().truth_tests, Some(false));
    assert!(!local
        .constraints
        .unwrap()
        .allowed_keywords
        .iter()
        .any(|word| word == "TRUE"));
    assert!(local.items.iter().all(|item| item.data.is_none()));
}

#[tokio::test]
async fn unresolved_types_keep_unknown_truth_knowledge_instead_of_claiming_boolean_support() {
    let (service, _) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    configure(server);
    let text = "SELECT TRIM('x') IS ";
    server.document_manager.update(URI.into(), text.into(), 7);
    let result = server
        .completion_snapshot(params(
            crate::position::lsp_position_at_end(text),
            json!({}),
        ))
        .await
        .unwrap();
    let constraint = result.constraints.unwrap();
    assert_eq!(constraint.truth_tests, None);
    assert!(constraint
        .allowed_keywords
        .iter()
        .any(|value| value == "TRUE"));
}
#[tokio::test]
async fn changed_scope_version_profile_and_invalid_positions_are_rejected() {
    let (service, _) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    configure(server);
    let text = "SELECT TRUE IS ";
    server.document_manager.update(URI.into(), text.into(), 7);
    let position = crate::position::lsp_position_at_end(text);
    for patch in [
        json!({"version":8}),
        json!({"oxideAnalysisToken":"old"}),
        json!({"dialect":"mysql"}),
        json!({"dialect":"redshift"}),
        json!({"position":{"line":99,"character":1}}),
    ] {
        assert!(
            server
                .completion_snapshot(params(position, patch.clone()))
                .await
                .is_err(),
            "{patch}"
        );
    }
    server.apply_configuration(json!({"oxideAnalysisToken":"new"}));
    assert!(server
        .completion_snapshot(params(position, json!({})))
        .await
        .is_err());
}

#[tokio::test]
async fn catalog_wire_caps_report_incompleteness_without_hiding_a_late_exact_field() {
    let (service, _) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    configure(server);
    let columns=(0..3000).map(|index|json!({"name":format!("field_{index:04}"),"data_type":"integer","nullable":false})).collect::<Vec<_>>();
    server.apply_configuration(json!({"oxideAnalysisToken":"confirmed","schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","tables":[{"name":"users","columns":columns}],"functions":[]}]}));
    let sql = "SELECT  FROM users";
    server.document_manager.update(URI.into(), sql.into(), 7);
    let result = server
        .completion_snapshot(params(Position::new(0, 7), json!({})))
        .await
        .unwrap();
    assert!(result.is_incomplete);
    assert!(result.items.len() <= 512);
    assert!(result.constraints.is_none());
    let prefix = "SELECT field_2999";
    let sql = prefix.to_owned() + " FROM users";
    server.document_manager.update(URI.into(), sql, 8);
    let result = server
        .completion_snapshot(params(
            crate::position::lsp_position_at_end(prefix),
            json!({"version":8}),
        ))
        .await
        .unwrap();
    assert!(result.items.iter().any(|item| item.label == "field_2999"));
}
