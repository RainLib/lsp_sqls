//! Deterministic scheduling around real PostgreSQL parsing/completion.
use super::*;
use crate::{
    dialect::{CompletionSnapshot, Dialect},
    dialects::PostgresDialect,
    server::SqlLspServer,
};
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::Notify;
use tower_lsp::lsp_types::*;
use tower_lsp::{LanguageServer, LspService};

struct PausedPostgres {
    inner: PostgresDialect,
    pause_completion: AtomicBool,
    pause_parse: AtomicBool,
    resume: Notify,
}
impl PausedPostgres {
    fn new(completion: bool, parse: bool) -> Self {
        Self {
            inner: PostgresDialect::new(),
            pause_completion: AtomicBool::new(completion),
            pause_parse: AtomicBool::new(parse),
            resume: Notify::new(),
        }
    }
    async fn pause(&self, flag: &AtomicBool) {
        if flag.swap(false, Ordering::SeqCst) {
            self.resume.notified().await;
        }
    }
}
#[async_trait::async_trait]
impl Dialect for PausedPostgres {
    fn name(&self) -> &str {
        "postgres"
    }
    async fn parse(&self, text: &str, schema: Option<&Schema>) -> Vec<Diagnostic> {
        self.inner.parse(text, schema).await
    }
    async fn parse_with_syntax(
        &self,
        text: &str,
        schema: Option<&Schema>,
    ) -> crate::parser::ParseResult {
        self.pause(&self.pause_parse).await;
        self.inner.parse_with_syntax(text, schema).await
    }
    async fn completion(
        &self,
        text: &str,
        position: Position,
        schema: Option<&Schema>,
    ) -> Vec<CompletionItem> {
        self.inner.completion(text, position, schema).await
    }
    async fn completion_with_scope(
        &self,
        text: &str,
        position: Position,
        schema: Option<&Schema>,
        context: Option<&CompletionContext>,
    ) -> CompletionSnapshot {
        self.pause(&self.pause_completion).await;
        self.inner
            .completion_with_scope(text, position, schema, context)
            .await
    }
    async fn hover(
        &self,
        text: &str,
        position: Position,
        schema: Option<&Schema>,
    ) -> Option<Hover> {
        self.inner.hover(text, position, schema).await
    }
    async fn goto_definition(
        &self,
        text: &str,
        position: Position,
        schema: Option<&Schema>,
    ) -> Option<Location> {
        self.inner.goto_definition(text, position, schema).await
    }
    async fn references(
        &self,
        text: &str,
        position: Position,
        schema: Option<&Schema>,
    ) -> Vec<Location> {
        self.inner.references(text, position, schema).await
    }
    async fn format(&self, text: &str) -> String {
        self.inner.format(text).await
    }
    async fn validate(&self, text: &str, schema: Option<&Schema>) -> Vec<Diagnostic> {
        self.inner.validate(text, schema).await
    }
}
fn configuration(uri: &str, schema: Schema) -> DidChangeConfigurationParams {
    DidChangeConfigurationParams {
        settings: json!({"schemas":[schema],"fileSchemas":{(uri):schema.id},"fileDialects":{(uri):"postgres"}}),
    }
}
fn completion(uri: &str, text: &str) -> CompletionParams {
    CompletionParams {
        text_document_position: TextDocumentPositionParams::new(
            TextDocumentIdentifier::new(Url::parse(uri).unwrap()),
            lsp_position_at_end(text),
        ),
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: None,
    }
}
fn seed(server: &SqlLspServer, uri: &str, text: &str) -> Schema {
    let mut schema = test_schema("app", &["ledger"]);
    schema.tables[0].columns = vec![Column {
        name: "old_value".into(),
        data_type: "int".into(),
        ..Default::default()
    }];
    server.schema_manager.register(schema.clone());
    server.configured_file_schemas.insert(uri.into(), schema.id);
    server
        .configured_file_dialects
        .insert(uri.into(), "postgres".into());
    server.document_manager.update(uri.into(), text.into(), 1);
    schema
}
#[tokio::test]
async fn configuration_cannot_replace_metadata_mid_completion_analysis() {
    let paused = Arc::new(PausedPostgres::new(true, false));
    let (service, _socket) = LspService::new(|client| {
        let mut server = SqlLspServer::new(client);
        Arc::get_mut(&mut server.dialect_registry)
            .unwrap()
            .register_alias("postgres", paused.clone());
        server
    });
    let server = service.inner();
    let uri = "oxide://analysis/coherent.sql";
    let text = "SELECT * FROM ledger l WHERE l.";
    let old = seed(server, uri, text);
    let mut next = old.clone();
    next.tables[0].columns[0].name = "new_value".into();
    let mut request = tokio_test::task::spawn(server.completion(completion(uri, text)));
    tokio_test::assert_pending!(request.poll());
    let mut update =
        tokio_test::task::spawn(server.did_change_configuration(configuration(uri, next)));
    let _ = update.poll();
    assert_eq!(
        server.schema_manager.get(old.id).unwrap().tables[0].columns[0].name,
        "old_value",
        "configuration changed while a real completion was suspended"
    );
    paused.resume.notify_one();
    let result = tokio_test::assert_ready!(request.poll()).unwrap().unwrap();
    let items = match result {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    };
    assert!(items.iter().any(|i| i.label == "old_value"), "{items:?}");
    assert!(!items.iter().any(|i| i.label == "new_value"));
    let _ = update.poll();
    assert_eq!(
        server.schema_manager.get(old.id).unwrap().tables[0].columns[0].name,
        "new_value"
    );
}

#[tokio::test]
async fn concurrent_readers_and_cancellation_do_not_starve_a_queued_configuration() {
    let paused = Arc::new(PausedPostgres::new(true, false));
    let (service, _socket) = LspService::new(|client| {
        let mut server = SqlLspServer::new(client);
        Arc::get_mut(&mut server.dialect_registry)
            .unwrap()
            .register_alias("postgres", paused.clone());
        server
    });
    let server = service.inner();
    let uri = "oxide://analysis/readers.sql";
    let text = "SELECT * FROM ledger l WHERE l.";
    let mut next = seed(server, uri, text);
    next.tables[0].columns[0].name = "new_value".into();
    let id = next.id;
    let mut first = tokio_test::task::spawn(server.completion(completion(uri, text)));
    tokio_test::assert_pending!(first.poll());
    // A second real completion runs while the first reader is suspended.
    let mut parallel = tokio_test::task::spawn(server.completion(completion(uri, text)));
    assert!(tokio_test::assert_ready!(parallel.poll())
        .unwrap()
        .is_some());
    let mut update =
        tokio_test::task::spawn(server.did_change_configuration(configuration(uri, next)));
    tokio_test::assert_pending!(update.poll());
    let mut later = tokio_test::task::spawn(server.completion(completion(uri, text)));
    tokio_test::assert_pending!(later.poll());
    drop(first); // Actual future cancellation must release its analysis guard.
    let _ = update.poll();
    assert_eq!(
        server.schema_manager.get(id).unwrap().tables[0].columns[0].name,
        "new_value"
    );
    let result = tokio_test::assert_ready!(later.poll()).unwrap().unwrap();
    let items = match result {
        CompletionResponse::Array(i) => i,
        CompletionResponse::List(i) => i.items,
    };
    assert!(items.iter().any(|i| i.label == "new_value"));
    assert!(!items.iter().any(|i| i.label == "old_value"));
}

#[tokio::test]
async fn queued_diagnostics_are_discarded_after_a_metadata_commit() {
    let (service, _socket) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    let uri = "oxide://analysis/queued.sql";
    let mut next = seed(server, uri, "SELECT old_value FROM ledger");
    next.tables[0].columns[0].name = "new_value".into();
    let sender = server.analysis.publication.lock().await;
    let mut old = tokio_test::task::spawn(server.publish_document_diagnostics(uri));
    tokio_test::assert_pending!(old.poll());
    let mut update =
        tokio_test::task::spawn(server.did_change_configuration(configuration(uri, next)));
    let _ = update.poll();
    assert_eq!(*server.analysis.configuration.read().await, 1);
    drop(sender);
    assert!(
        !tokio_test::assert_ready!(old.poll()),
        "stale metadata diagnostics were sent"
    );
}

async fn stale_parse_after_document_replacement(reopen: bool) {
    let paused = Arc::new(PausedPostgres::new(false, true));
    let (service, _socket) = LspService::new(|client| {
        let mut server = SqlLspServer::new(client);
        Arc::get_mut(&mut server.dialect_registry)
            .unwrap()
            .register_alias("postgres", paused.clone());
        server
    });
    let server = service.inner();
    let uri = "oxide://analysis/stale.sql";
    seed(server, uri, "SELECT old_value FROM ledger");
    let mut old = tokio_test::task::spawn(server.publish_document_diagnostics(uri));
    tokio_test::assert_pending!(old.poll());
    if reopen {
        server.document_manager.remove(uri);
    }
    server.document_manager.update(
        uri.into(),
        if reopen {
            "SELECT old_value FROM ledger".into()
        } else {
            "SELECT 1".into()
        },
        if reopen { 1 } else { 2 },
    );
    paused.resume.notify_one();
    assert!(!tokio_test::assert_ready!(old.poll()));
}

#[tokio::test]
async fn diagnostics_do_not_label_old_sql_with_a_new_document_version() {
    stale_parse_after_document_replacement(false).await;
}

#[tokio::test]
async fn diagnostics_do_not_publish_into_a_reopened_same_version_document() {
    stale_parse_after_document_replacement(true).await;
}

#[tokio::test]
async fn diagnostics_blocked_on_client_io_do_not_hold_the_configuration_gate() {
    use tower::Service;
    let (mut service, _socket) = LspService::new(SqlLspServer::new);
    let initialized = service
        .call(
            tower_lsp::jsonrpc::Request::build("initialize")
                .id(1)
                .params(json!({"capabilities":{}}))
                .finish(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(initialized.error().is_none());
    let server = service.inner();
    let uri = "oxide://analysis/backpressure.sql";
    let mut next = seed(server, uri, "SELECT 1");
    next.tables[0].columns[0].name = "new_value".into();
    let id = next.id;
    let mut fill = tokio_test::task::spawn(async {
        for _ in 0..16 {
            server.client.log_message(MessageType::INFO, "fill").await;
        }
    });
    tokio_test::assert_pending!(fill.poll());
    let mut publication = tokio_test::task::spawn(server.publish_document_diagnostics(uri));
    tokio_test::assert_pending!(publication.poll());
    let mut update =
        tokio_test::task::spawn(server.did_change_configuration(configuration(uri, next)));
    tokio_test::assert_pending!(update.poll()); // Logs blocked, state already committed.
    assert_eq!(
        server.schema_manager.get(id).unwrap().tables[0].columns[0].name,
        "new_value"
    );
    assert!(server.analysis.configuration.try_write().is_ok());
}

#[tokio::test]
async fn incremental_changes_commit_one_final_unicode_document_snapshot() {
    let (service, _socket) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    let uri = "oxide://analysis/changes.sql";
    seed(server, uri, "/* 😀 */ SELECT 1;");
    let mut change = tokio_test::task::spawn(server.did_change(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier::new(Url::parse(uri).unwrap(), 2),
        content_changes: vec![
            TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 16), Position::new(0, 17))),
                range_length: None,
                text: "2".into(),
            },
            TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 18), Position::new(0, 18))),
                range_length: None,
                text: " SELECT 3;".into(),
            },
        ],
    }));
    let _ = change.poll();
    let snapshot = server.document_manager.snapshot(uri).unwrap();
    assert_eq!(snapshot.text, "/* 😀 */ SELECT 2; SELECT 3;");
    assert_eq!(snapshot.version, 2);
}

fn snapshot_request(
    uri: &str,
    mode: super::super::diagnostic_snapshot::DiagnosticSnapshotMode,
) -> super::super::diagnostic_snapshot::DiagnosticSnapshotParams {
    super::super::diagnostic_snapshot::DiagnosticSnapshotParams {
        text_document: TextDocumentIdentifier {
            uri: uri.parse().unwrap(),
        },
        version: 1,
        dialect: "postgres".into(),
        mode,
        oxide_analysis_token: Some("current".into()),
    }
}
#[tokio::test]
async fn syntax_snapshot_does_not_hold_the_configuration_writer() {
    use super::super::diagnostic_snapshot::DiagnosticSnapshotMode;
    let paused = Arc::new(PausedPostgres::new(false, true));
    let (service, _socket) = LspService::new(|client| {
        let mut server = SqlLspServer::new(client);
        Arc::get_mut(&mut server.dialect_registry)
            .unwrap()
            .register_alias("postgres", paused.clone());
        server
    });
    let server = service.inner();
    let uri = "oxide://snapshot/unblocked.sql";
    seed(server, uri, "SELECT 1");
    let mut request = tokio_test::task::spawn(
        server.document_diagnostic_snapshot(snapshot_request(uri, DiagnosticSnapshotMode::Syntax)),
    );
    tokio_test::assert_pending!(request.poll());
    assert!(
        server.analysis.configuration.try_write().is_ok(),
        "syntax parsing must not block metadata replacement"
    );
    paused.resume.notify_one();
    let response = tokio_test::assert_ready!(request.poll()).unwrap();
    assert_eq!(response.mode, DiagnosticSnapshotMode::Syntax);
}
#[tokio::test]
async fn syntax_snapshot_rejects_a_changed_document_while_parsing() {
    use super::super::diagnostic_snapshot::DiagnosticSnapshotMode;
    let paused = Arc::new(PausedPostgres::new(false, true));
    let (service, _socket) = LspService::new(|client| {
        let mut server = SqlLspServer::new(client);
        Arc::get_mut(&mut server.dialect_registry)
            .unwrap()
            .register_alias("postgres", paused.clone());
        server
    });
    let server = service.inner();
    let uri = "oxide://snapshot/changed.sql";
    seed(server, uri, "SELECT 1");
    let mut request = tokio_test::task::spawn(
        server.document_diagnostic_snapshot(snapshot_request(uri, DiagnosticSnapshotMode::Syntax)),
    );
    tokio_test::assert_pending!(request.poll());
    server
        .document_manager
        .update(uri.into(), "SELECT 2".into(), 2);
    paused.resume.notify_one();
    assert!(tokio_test::assert_ready!(request.poll()).is_err());
}
#[tokio::test]
async fn cancellation_of_a_metadata_snapshot_releases_the_configuration_gate() {
    use super::super::diagnostic_snapshot::DiagnosticSnapshotMode;
    let paused = Arc::new(PausedPostgres::new(false, true));
    let (service, _socket) = LspService::new(|client| {
        let mut server = SqlLspServer::new(client);
        Arc::get_mut(&mut server.dialect_registry)
            .unwrap()
            .register_alias("postgres", paused.clone());
        server
    });
    let server = service.inner();
    let uri = "oxide://snapshot/cancel.sql";
    seed(server, uri, "SELECT 1");
    *server.analysis.provenance.write().unwrap() = Some("current".into());
    let mut request = tokio_test::task::spawn(
        server
            .document_diagnostic_snapshot(snapshot_request(uri, DiagnosticSnapshotMode::Metadata)),
    );
    tokio_test::assert_pending!(request.poll());
    assert!(server.analysis.configuration.try_write().is_err());
    drop(request);
    assert!(server.analysis.configuration.try_write().is_ok());
}
