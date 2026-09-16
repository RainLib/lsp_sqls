//! Completion resolve for the SQL language server.
use super::limits::{
    COMPLETION_RESOLVE_CACHE_MAX_BYTES, COMPLETION_RESOLVE_CACHE_MAX_ENTRIES,
    COMPLETION_RESOLVE_DOCUMENTATION_MAX_BYTES, COMPLETION_RESOLVE_ID_FIELD,
    COMPLETION_RESOLVE_ORIGINAL_DATA_FIELD,
};
use serde_json::Map as JsonMap;
use serde_json::Value;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use tower_lsp::lsp_types::*;
#[derive(Clone)]
pub(super) struct CompletionResolveEntry {
    pub(super) documentation: Documentation,
    pub(super) bytes: usize,
}

#[derive(Default)]
pub(super) struct CompletionResolveCache {
    pub(super) entries: HashMap<u64, CompletionResolveEntry>,
    pub(super) order: VecDeque<u64>,
    pub(super) bytes: usize,
}

impl CompletionResolveCache {
    pub(super) fn insert(&mut self, id: u64, documentation: Documentation) -> bool {
        let Ok(bytes) = serde_json::to_vec(&documentation).map(|encoded| encoded.len()) else {
            return false;
        };
        if bytes > COMPLETION_RESOLVE_DOCUMENTATION_MAX_BYTES
            || bytes > COMPLETION_RESOLVE_CACHE_MAX_BYTES
        {
            return false;
        }

        while self.entries.len() >= COMPLETION_RESOLVE_CACHE_MAX_ENTRIES
            || self.bytes.saturating_add(bytes) > COMPLETION_RESOLVE_CACHE_MAX_BYTES
        {
            let Some(expired_id) = self.order.pop_front() else {
                break;
            };
            if let Some(expired) = self.entries.remove(&expired_id) {
                self.bytes = self.bytes.saturating_sub(expired.bytes);
            }
        }

        self.bytes = self.bytes.saturating_add(bytes);
        self.order.push_back(id);
        self.entries.insert(
            id,
            CompletionResolveEntry {
                documentation,
                bytes,
            },
        );
        true
    }

    pub(super) fn documentation(&self, id: u64) -> Option<Documentation> {
        self.entries
            .get(&id)
            .map(|entry| entry.documentation.clone())
    }
}

pub(super) fn completion_resolve_data(id: u64, original_data: Option<Value>) -> Value {
    let mut data = JsonMap::new();
    data.insert(COMPLETION_RESOLVE_ID_FIELD.to_string(), Value::from(id));
    if let Some(original_data) = original_data {
        data.insert(
            COMPLETION_RESOLVE_ORIGINAL_DATA_FIELD.to_string(),
            original_data,
        );
    }
    Value::Object(data)
}

pub(super) fn completion_resolve_identity(data: Option<&Value>) -> Option<u64> {
    data?.get(COMPLETION_RESOLVE_ID_FIELD)?.as_u64()
}

pub(super) fn original_completion_data(data: Option<&Value>) -> Option<Value> {
    data?.get(COMPLETION_RESOLVE_ORIGINAL_DATA_FIELD).cloned()
}

pub(super) fn defer_completion_documentation(
    items: &mut [CompletionItem],
    cache: &Mutex<CompletionResolveCache>,
    next_id: &AtomicU64,
) {
    let Ok(mut cache) = cache.lock() else {
        return;
    };
    for item in items {
        let Some(documentation) = item.documentation.take() else {
            continue;
        };
        let id = next_id.fetch_add(1, Ordering::Relaxed);
        if cache.insert(id, documentation.clone()) {
            item.data = Some(completion_resolve_data(id, item.data.take()));
        } else {
            item.documentation = Some(documentation);
        }
    }
}

pub(super) fn resolve_completion_documentation(
    mut item: CompletionItem,
    cache: &Mutex<CompletionResolveCache>,
) -> CompletionItem {
    let Some(id) = completion_resolve_identity(item.data.as_ref()) else {
        return item;
    };
    let original_data = original_completion_data(item.data.as_ref());
    if let Ok(cache) = cache.lock() {
        if let Some(documentation) = cache.documentation(id) {
            item.documentation = Some(documentation);
        }
    }
    item.data = original_data;
    item
}

pub(super) fn client_supports_completion_documentation_resolve(params: &InitializeParams) -> bool {
    params
        .capabilities
        .text_document
        .as_ref()
        .and_then(|capabilities| capabilities.completion.as_ref())
        .and_then(|capabilities| capabilities.completion_item.as_ref())
        .and_then(|capabilities| capabilities.resolve_support.as_ref())
        .is_some_and(|support| {
            support
                .properties
                .iter()
                .any(|property| property == "documentation")
        })
}
