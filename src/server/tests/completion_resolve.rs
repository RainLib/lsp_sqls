use super::*;

#[test]
fn completion_documentation_is_deferred_only_for_capable_clients() {
    let capable: InitializeParams = serde_json::from_value(serde_json::json!({
        "capabilities": {
            "textDocument": {
                "completion": {
                    "completionItem": {
                        "resolveSupport": {
                            "properties": ["documentation", "detail"]
                        }
                    }
                }
            }
        }
    }))
    .unwrap();
    let incapable: InitializeParams = serde_json::from_value(serde_json::json!({
        "capabilities": {
            "textDocument": {
                "completion": {
                    "completionItem": {
                        "resolveSupport": { "properties": ["detail"] }
                    }
                }
            }
        }
    }))
    .unwrap();

    assert!(client_supports_completion_documentation_resolve(&capable));
    assert!(!client_supports_completion_documentation_resolve(
        &incapable
    ));
}

#[test]
fn completion_documentation_is_deferred_and_restores_original_data() {
    let cache = Mutex::new(CompletionResolveCache::default());
    let next_id = AtomicU64::new(1);
    let original_data = serde_json::json!({ "source": "metadata" });
    let mut items = vec![CompletionItem {
        label: "customer_id".to_string(),
        documentation: Some(Documentation::String("Primary key".to_string())),
        data: Some(original_data.clone()),
        ..Default::default()
    }];

    defer_completion_documentation(&mut items, &cache, &next_id);
    assert!(items[0].documentation.is_none());
    assert_eq!(
        items[0]
            .data
            .as_ref()
            .and_then(|data| data.get("oxideSqlLspCompletionResolveId"))
            .and_then(serde_json::Value::as_u64),
        Some(1)
    );

    let resolved = resolve_completion_documentation(items.remove(0), &cache);
    assert_eq!(
        resolved.documentation,
        Some(Documentation::String("Primary key".to_string()))
    );
    assert_eq!(resolved.data, Some(original_data));
}

#[test]
fn completion_documentation_cache_is_bounded_and_oversize_docs_stay_inline() {
    let cache = Mutex::new(CompletionResolveCache::default());
    let next_id = AtomicU64::new(1);
    let mut oversize = vec![CompletionItem {
        label: "large".to_string(),
        documentation: Some(Documentation::String(
            "x".repeat(COMPLETION_RESOLVE_DOCUMENTATION_MAX_BYTES + 1),
        )),
        ..Default::default()
    }];
    defer_completion_documentation(&mut oversize, &cache, &next_id);
    assert!(oversize[0].documentation.is_some());
    assert!(oversize[0].data.is_none());

    let mut cache = cache.lock().unwrap();
    for id in 0..=COMPLETION_RESOLVE_CACHE_MAX_ENTRIES as u64 {
        assert!(cache.insert(id, Documentation::String("doc".to_string())));
    }
    assert_eq!(cache.entries.len(), COMPLETION_RESOLVE_CACHE_MAX_ENTRIES);
    assert!(!cache.entries.contains_key(&0));
    assert!(cache
        .entries
        .contains_key(&(COMPLETION_RESOLVE_CACHE_MAX_ENTRIES as u64)));
}
