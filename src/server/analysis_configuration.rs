//! Read back committed configuration provenance without opening a document.
use super::runtime::SqlLspServer;
use tower_lsp::jsonrpc::{Error, Result};

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisConfigurationParams {
    pub oxide_analysis_token: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisConfigurationReceipt {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oxide_analysis_token: Option<String>,
}

impl SqlLspServer {
    pub async fn analysis_configuration(
        &self,
        params: AnalysisConfigurationParams,
    ) -> Result<AnalysisConfigurationReceipt> {
        let value = serde_json::Value::String(params.oxide_analysis_token);
        let expected = super::analysis_provenance::parse_analysis_token(Some(&value))
            .ok_or_else(|| Error::invalid_params("Invalid configuration token"))?;
        // Pair the token with the complete commit, including schema and routing.
        let _configuration = self.analysis.configuration.read().await;
        let applied = self
            .analysis
            .provenance
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .as_deref()
            == Some(expected.as_str());
        Ok(AnalysisConfigurationReceipt {
            applied,
            oxide_analysis_token: applied.then_some(expected),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tower_lsp::LspService;

    async fn receipt(server: &SqlLspServer, token: &str) -> serde_json::Value {
        serde_json::to_value(
            server
                .analysis_configuration(AnalysisConfigurationParams {
                    oxide_analysis_token: token.into(),
                })
                .await
                .unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_fresh_process_cannot_confirm_an_old_token() {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        assert_eq!(
            receipt(service.inner(), "old").await,
            json!({"applied": false})
        );
    }

    #[tokio::test]
    async fn confirms_configuration_without_opening_any_documents() {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        server.apply_configuration(json!({"oxideAnalysisToken":"current"}));
        assert_eq!(
            receipt(server, "current").await,
            json!({"applied":true,"oxideAnalysisToken":"current"})
        );
    }

    #[tokio::test]
    async fn a_replacement_never_echoes_the_other_context_token() {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        server.apply_configuration(json!({"oxideAnalysisToken":"first"}));
        server.apply_configuration(json!({"oxideAnalysisToken":"second"}));
        assert_eq!(receipt(server, "first").await, json!({"applied":false}));
        assert_eq!(receipt(server, "second").await["applied"], true);
    }

    #[tokio::test]
    async fn legacy_or_invalid_configuration_revokes_the_receipt() {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        for replacement in [json!({}), json!({"oxideAnalysisToken":"invalid token"})] {
            server.apply_configuration(json!({"oxideAnalysisToken":"current"}));
            server.apply_configuration(replacement);
            assert_eq!(receipt(server, "current").await, json!({"applied":false}));
        }
    }

    #[tokio::test]
    async fn rejects_malformed_or_unbounded_tokens() {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        for token in ["".into(), "a b".into(), "列".into(), "x".repeat(129)] {
            let error = service
                .inner()
                .analysis_configuration(AnalysisConfigurationParams {
                    oxide_analysis_token: token,
                })
                .await
                .unwrap_err();
            assert_eq!(error.code, tower_lsp::jsonrpc::ErrorCode::InvalidParams);
        }
    }

    #[tokio::test]
    async fn accepts_the_bounded_token_without_a_document_or_dialect() {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        let token = "x".repeat(128);
        service
            .inner()
            .apply_configuration(json!({"oxideAnalysisToken":token}));
        assert_eq!(receipt(service.inner(), &token).await["applied"], true);
    }

    #[tokio::test]
    async fn waits_for_the_complete_configuration_commit() {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        let gate = server.analysis.configuration.write().await;
        server.apply_configuration(json!({"oxideAnalysisToken":"current"}));
        let mut pending =
            tokio_test::task::spawn(server.analysis_configuration(AnalysisConfigurationParams {
                oxide_analysis_token: "current".into(),
            }));
        tokio_test::assert_pending!(pending.poll());
        drop(gate);
        assert!(tokio_test::assert_ready!(pending.poll()).unwrap().applied);
        assert!(server.analysis.configuration.try_write().is_ok());
    }

    #[tokio::test]
    async fn cancelling_a_waiting_receipt_does_not_poison_the_writer() {
        let (service, _socket) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        let gate = server.analysis.configuration.write().await;
        let mut pending =
            tokio_test::task::spawn(server.analysis_configuration(AnalysisConfigurationParams {
                oxide_analysis_token: "current".into(),
            }));
        tokio_test::assert_pending!(pending.poll());
        drop(pending);
        drop(gate);
        assert!(server.analysis.configuration.try_write().is_ok());
    }

    fn valid_configuration() -> serde_json::Value {
        json!({"oxideAnalysisToken":"old","defaultDialect":"postgres","schemas":[{"id":"00000000-0000-4000-8000-000000000001","database":"old","tables":[],"functions":[]}],"fileSchemas":{"oxide://query/current.sql":"00000000-0000-4000-8000-000000000001"},"fileDialects":{"oxide://query/current.sql":"postgres"},"completionPreferences":{"keywordCase":"upper","tableAlias":"none"},"formattingPreferences":{"keywordCase":"upper"}})
    }

    #[tokio::test]
    async fn malformed_schema_cannot_confirm_a_new_context_or_retain_old_authority() {
        let (service, _) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        server.apply_configuration(valid_configuration());
        assert_eq!(receipt(server, "old").await["applied"], true);
        let mut invalid = valid_configuration();
        invalid["oxideAnalysisToken"] = json!("new");
        invalid["schemas"][0]
            .as_object_mut()
            .unwrap()
            .remove("functions");
        server.apply_configuration(invalid);
        assert_eq!(receipt(server, "new").await, json!({"applied":false}));
        assert_eq!(receipt(server, "old").await, json!({"applied":false}));
    }

    #[tokio::test]
    async fn invalid_payloads_leave_the_last_valid_settings_intact_and_revoke_receipts() {
        for invalid in [
            json!(null),
            json!([]),
            json!(true),
            json!({"oxideAnalysisToken":"new","schemas":null}),
            json!({"oxideAnalysisToken":"new","defaultDialect":"missing"}),
            json!({"oxideAnalysisToken":"new","completionPreferences":{"keywordCase":"auto"}}),
            json!({"oxideAnalysisToken":"new","fileSchemas":{"oxide://query/current.sql":null}}),
            json!({"oxideAnalysisToken":"new","fileDialects":{"oxide://query/current.sql":"missing"}}),
        ] {
            let (service, _) = LspService::new(SqlLspServer::new);
            let server = service.inner();
            server.apply_configuration(valid_configuration());
            server.apply_configuration(invalid.clone());
            assert_eq!(receipt(server, "old").await["applied"], false, "{invalid}");
            assert_eq!(receipt(server, "new").await["applied"], false, "{invalid}");
            assert_eq!(server.schema_manager.list_ids().len(), 1, "{invalid}");
            assert_eq!(
                server.default_dialect.read().unwrap().as_str(),
                "postgres",
                "{invalid}"
            );
            assert_eq!(server.configured_file_schemas.len(), 1, "{invalid}");
            assert_eq!(server.configured_file_dialects.len(), 1, "{invalid}");
        }
    }

    #[tokio::test]
    async fn a_rejected_update_never_partially_changes_dialect_preferences_or_catalogs() {
        let (service, _) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        server.apply_configuration(valid_configuration());
        let mut invalid = valid_configuration();
        invalid["oxideAnalysisToken"] = json!("new");
        invalid["defaultDialect"] = json!("mysql");
        invalid["completionPreferences"]["keywordCase"] = json!("lower");
        invalid["formattingPreferences"]["keywordCase"] = json!("lower");
        invalid["schemas"] = json!("malformed");
        invalid["fileSchemas"] = json!({});
        invalid["fileDialects"] = json!({});
        server.apply_configuration(invalid);
        assert_eq!(server.default_dialect.read().unwrap().as_str(), "postgres");
        assert!(server
            .completion_preferences
            .read()
            .unwrap()
            .keyword_case
            .uppercase_option()
            .unwrap());
        assert!(server
            .formatting_preferences
            .read()
            .unwrap()
            .keyword_uppercase()
            .unwrap());
        assert_eq!(server.configured_file_schemas.len(), 1);
        assert_eq!(server.configured_file_dialects.len(), 1);
        assert_eq!(receipt(server, "new").await["applied"], false);
    }

    #[tokio::test]
    async fn explicit_invalid_fields_never_become_an_accepted_noop() {
        for patch in [
            json!({"oxideAnalysisToken":null}),
            json!({"oxideAnalysisToken":3}),
            json!({"defaultDialect":null}),
            json!({"defaultDialect":3}),
            json!({"defaultDialect":"x".repeat(1000)}),
            json!({"schemas":{}}),
            json!({"schemas":[{"id":"invalid","database":"bad","tables":[],"functions":[]}]}),
            json!({"completionPreferences":null}),
            json!({"completionPreferences":{"tableAlias":"invalid"}}),
            json!({"formattingPreferences":null}),
            json!({"formattingPreferences":{"fromClauseLayout":"invalid"}}),
            json!({"fileSchemas":null}),
            json!({"fileSchemas":{"oxide://query/current.sql":"not-a-uuid"}}),
            json!({"fileDialects":null}),
            json!({"fileDialects":{"oxide://query/current.sql":4}}),
        ] {
            let (service, _) = LspService::new(SqlLspServer::new);
            let server = service.inner();
            server.apply_configuration(valid_configuration());
            let mut invalid = json!({"oxideAnalysisToken":"new"});
            invalid
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            server.apply_configuration(invalid.clone());
            assert_eq!(receipt(server, "old").await["applied"], false, "{invalid}");
            assert_eq!(receipt(server, "new").await["applied"], false, "{invalid}");
            assert_eq!(server.schema_manager.list_ids().len(), 1, "{invalid}");
        }
    }

    #[tokio::test]
    async fn duplicate_schema_ids_reject_the_entire_update() {
        let (service, _) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        server.apply_configuration(valid_configuration());
        let mut invalid = valid_configuration();
        invalid["oxideAnalysisToken"] = json!("new");
        let mut duplicate = invalid["schemas"][0].clone();
        duplicate["database"] = json!("other");
        invalid["schemas"].as_array_mut().unwrap().push(duplicate);
        server.apply_configuration(invalid);
        assert_eq!(receipt(server, "new").await["applied"], false);
        let schema = server
            .schema_manager
            .get("00000000-0000-4000-8000-000000000001".parse().unwrap())
            .unwrap();
        assert_eq!(schema.database, "old");
    }

    #[tokio::test]
    async fn valid_repair_and_empty_catalog_have_fresh_receipts() {
        let (service, _) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        server.apply_configuration(valid_configuration());
        server.apply_configuration(json!({"oxideAnalysisToken":"bad","schemas":null}));
        let mut repaired = valid_configuration();
        repaired["oxideAnalysisToken"] = json!("repaired");
        repaired["schemas"][0]["database"] = json!("fresh");
        server.apply_configuration(repaired);
        assert_eq!(receipt(server, "repaired").await["applied"], true);
        assert_eq!(receipt(server, "old").await["applied"], false);
        server.apply_configuration(
            json!({"oxideAnalysisToken":"clear","schemas":[],"fileSchemas":{},"fileDialects":{}}),
        );
        assert_eq!(receipt(server, "clear").await["applied"], true);
        assert!(server.schema_manager.list_ids().is_empty());
        assert!(server.configured_file_schemas.is_empty());
    }

    #[tokio::test]
    async fn registered_aliases_normalize_without_losing_their_identity() {
        let (service, _) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        server.apply_configuration(json!({"oxideAnalysisToken":"alias","defaultDialect":"PostgreSQL","fileDialects":{"oxide://query/current.sql":"MSSQL"}}));
        assert_eq!(receipt(server, "alias").await["applied"], true);
        assert_eq!(
            server.default_dialect.read().unwrap().as_str(),
            "postgresql"
        );
        assert_eq!(
            server
                .configured_file_dialects
                .get("oxide://query/current.sql")
                .unwrap()
                .as_str(),
            "mssql"
        );
    }

    #[tokio::test]
    async fn validation_warnings_never_echo_payload_values() {
        let (service, _) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        let messages = server.apply_configuration(
            json!({"oxideAnalysisToken":"new","defaultDialect":"credential-payload"}),
        );
        assert!(messages
            .iter()
            .all(|(_, message)| !message.contains("credential-payload")));
        assert_eq!(receipt(server, "new").await["applied"], false);
    }

    #[tokio::test]
    async fn rejection_blocks_metadata_snapshots_but_keeps_document_semantics_available() {
        let (service, _) = LspService::new(SqlLspServer::new);
        let server = service.inner();
        server.apply_configuration(valid_configuration());
        let uri = "oxide://query/current.sql";
        let text = "WITH q AS (SELECT 1 AS local_id) SELECT q.lo FROM q;";
        server.document_manager.update(uri.into(), text.into(), 7);
        let metadata = |token: &str| {
            serde_json::from_value(json!({"textDocument":{"uri":uri},"version":7,"dialect":"postgres","mode":"metadata","oxideAnalysisToken":token})).unwrap()
        };
        assert!(server
            .document_diagnostic_snapshot(metadata("old"))
            .await
            .is_ok());
        server.apply_configuration(json!({"oxideAnalysisToken":"new","schemas":null}));
        for token in ["old", "new"] {
            assert!(server
                .document_diagnostic_snapshot(metadata(token))
                .await
                .is_err());
        }
        let local=server.document_completion_snapshot(serde_json::from_value(json!({"textDocument":{"uri":uri},"version":7,"dialect":"postgres","position":{"line":0,"character":text.find("q.lo").unwrap()+4},"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).unwrap()).await.unwrap();
        assert!(local.items.iter().any(|item| item.label == "local_id"));
        assert_eq!(server.schema_manager.list_ids().len(), 1);
    }
}
