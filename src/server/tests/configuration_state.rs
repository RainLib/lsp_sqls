use super::*;
use crate::server::SqlLspServer;
use serde_json::json;
use tower_lsp::lsp_types::{DidChangeConfigurationParams, MessageType};
use tower_lsp::{LanguageServer, LspService};

#[tokio::test]
async fn configuration_applies_all_routing_before_waiting_for_client_logs() {
    let (service, _socket) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    let uri = "oxide://query/config.mysql.sql";
    server
        .configured_file_dialects
        .insert(uri.into(), "postgres".into());
    server
        .inferred_file_dialects
        .insert(uri.into(), "mysql".into());
    server.document_languages.insert(uri.into(), "sql".into());
    let client = server.client.clone();
    // Keep the outgoing socket unread and fill its bounded queue. The first
    // configuration log must suspend, without a sleep or scheduler assumption.
    let mut blocked_logs = tokio_test::task::spawn(async move {
        for _ in 0..16 {
            client
                .log_message(MessageType::INFO, "fill outgoing queue")
                .await;
        }
    });
    tokio_test::assert_pending!(blocked_logs.poll());
    let schema = test_schema("next", &["users"]);
    let id = schema.id;
    let mut update = tokio_test::task::spawn(server.did_change_configuration(
        DidChangeConfigurationParams {
            settings: json!({
                "defaultDialect":"mysql", "schemas":[schema],
                "fileSchemas":{(uri):id}, "fileDialects":{(uri):"tsql"}
            }),
        },
    ));
    tokio_test::assert_pending!(update.poll());
    assert_eq!(server.default_dialect_name(), "mysql");
    assert!(server.schema_manager.get(id).is_some());
    assert_eq!(
        server.configured_file_schemas.get(uri).map(|entry| *entry),
        Some(id)
    );
    assert_eq!(
        server.dialect_identity_for_file(uri).as_deref(),
        Some("tsql")
    );
    assert!(!server.inferred_file_dialects.contains_key(uri));
}
