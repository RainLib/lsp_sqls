//! Auxiliary editor buffers inherit only an explicit query-document route.
//! Their text, versions and lifecycle remain independent of the source buffer.
use tower_lsp::lsp_types::Url;

pub(super) fn configured_source_uri(uri: &str) -> Option<String> {
    if uri.len() > 8192 {
        return None;
    }
    let document = Url::parse(uri).ok()?;
    if document.scheme() != "oxide" || document.host_str() != Some("where") {
        return None;
    }
    let mut parameters = document.query_pairs();
    let (name, source) = parameters.next()?;
    if name != "source" || parameters.next().is_some() {
        return None;
    }
    let source_uri = Url::parse(&source).ok()?;
    if source_uri.scheme() != "oxide"
        || source_uri.host_str() != Some("query")
        || source_uri.query().is_some()
        || source_uri.fragment().is_some()
        || !source_uri.username().is_empty()
        || source_uri.password().is_some()
        || source_uri.port().is_some()
        || source_uri.path().trim_matches('/').is_empty()
    {
        return None;
    }
    Some(source_uri.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_bounded_explicit_query_routes_can_be_inherited() {
        let valid = "oxide://where/owner.sql?source=oxide%3A%2F%2Fquery%2Ftab.mysql.sql";
        assert_eq!(
            configured_source_uri(valid).as_deref(),
            Some("oxide://query/tab.mysql.sql")
        );
        for uri in [
            "oxide://query/tab.sql?source=oxide://query/other.sql",
            "oxide://where/x.sql?source=file:///secret.sql",
            "oxide://where/x.sql?source=oxide://where/other.sql",
            "oxide://where/x.sql?source=oxide://query/",
            "oxide://where/x.sql?source=oxide://query/a.sql%3Fx=1",
            "oxide://where/x.sql?source=oxide://query/a.sql%23x",
            "oxide://where/x.sql?source=oxide://query/a.sql&source=oxide://query/b.sql",
            "oxide://where/x.sql?source=oxide://user:password@query/a.sql",
        ] {
            assert_eq!(configured_source_uri(uri), None, "{uri}");
        }
        assert_eq!(
            configured_source_uri(&format!("{valid}{}", "a".repeat(8192))),
            None
        );
    }

    #[tokio::test]
    async fn auxiliary_buffers_inherit_live_explicit_routes_without_touching_source_text() {
        use super::super::runtime::SqlLspServer;
        use serde_json::json;
        let (service, _) = tower_lsp::LspService::new(SqlLspServer::new);
        let server = service.inner();
        let source = "oxide://query/tab.mysql.sql";
        let uri = "oxide://where/owner.mysql.sql?source=oxide%3A%2F%2Fquery%2Ftab.mysql.sql";
        let id = "00000000-0000-4000-8000-000000000001";
        server.apply_configuration(json!({"defaultDialect":"mysql","schemas":[{"id":id,"database":"app","tables":[],"functions":[]}],"fileSchemas":{source:id},"fileDialects":{source:"sqlserver"}}));
        server
            .document_manager
            .update(source.into(), "SELECT main_text".into(), 9);
        server
            .document_manager
            .update(uri.into(), "SELECT auxiliary_text".into(), 1);
        server.ensure_dialect_for_document(uri, "mysql");
        assert_eq!(
            server.dialect_identity_for_file(uri).as_deref(),
            Some("sqlserver")
        );
        assert_eq!(
            server.get_dialect_for_file(uri).unwrap().name(),
            "sqlserver"
        );
        assert_eq!(server.get_schema_for_file(uri).unwrap().database, "app");
        server.apply_configuration(json!({"fileDialects":{source:"postgres"},"fileSchemas":{}}));
        assert_eq!(
            server.dialect_identity_for_file(uri).as_deref(),
            Some("postgres")
        );
        assert!(server.get_schema_for_file(uri).is_none());
        assert_eq!(
            server.document_manager.get(source).as_deref(),
            Some("SELECT main_text")
        );
        server.apply_configuration(json!({"fileDialects":{source:"postgres",uri:"sqlite"}}));
        assert_eq!(
            server.dialect_identity_for_file(uri).as_deref(),
            Some("sqlite")
        );
    }
}
