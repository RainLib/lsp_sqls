use super::*;

#[test]
fn rewrites_sql_and_json_current_document_sentinels() {
    let document_uri = Url::parse("untitled:OxideStudio/%E6%9F%A5%E8%AF%A2-1").unwrap();
    let mut locations = vec![
        test_location("file:///current.sql"),
        test_location("file:///current.json"),
    ];

    rewrite_current_document_location_uris(&mut locations, &document_uri);

    assert!(locations
        .iter()
        .all(|location| location.uri == document_uri));
}

#[test]
fn preserves_schema_and_other_source_uris() {
    let document_uri = Url::parse("file:///workspace/query.sql").unwrap();
    let source_uri = Url::parse("file:///schemas/app.sql").unwrap();
    let mut schema_fallback = test_location("file:///schema.sql");
    let mut schema_source = Location {
        uri: source_uri.clone(),
        range: Range::default(),
    };

    rewrite_current_document_location_uri(&mut schema_fallback, &document_uri);
    rewrite_current_document_location_uri(&mut schema_source, &document_uri);

    assert_eq!(schema_fallback.uri.as_str(), "file:///schema.sql");
    assert_eq!(schema_source.uri, source_uri);
}
